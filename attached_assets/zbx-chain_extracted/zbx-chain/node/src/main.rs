//! zbx-node: Zebvix full node entrypoint.
//!
//! Usage:
//!   zbx-node [--network mainnet|testnet] [--config <path>] [--validator]
//!            [--data-dir <dir>] [--rpc-port <port>] [--bind-addr <ip>]
//!
//! The node joins the Zebvix mainnet (chain id 8989, RPC 8545) or testnet
//! (chain id 8990, RPC 18545), syncs from genesis, and optionally
//! participates in HotStuff-BFT consensus when a validator key is provided.

mod block_producer;
mod consensus;
mod config;
mod genesis;
mod network;
mod noise; // SEC-2026-05-09 (P1+P2): Noise XX transport encryption + crypto PeerId
mod node;
mod readiness; // Task #14: replaces Pass-12 mainnet boot-panic guard
// Task #22: snapshot manifest verification at fast-sync import boundary.
// `snapshot_import` lives in the lib facade (`src/lib.rs`) so the
// integration tests under `node/tests/` can exercise it without
// touching the binary's private item tree.
use zbx_node::snapshot_import;

use clap::Parser;
use config::NodeConfig;
use genesis::{GenesisConfig, Network};
use node::ZbxNode;
use std::path::PathBuf;
use std::process::ExitCode;
use tracing::{error, info};
use tracing_subscriber::{fmt, EnvFilter};

#[derive(Debug, Parser)]
#[command(
    name = "zbx-node",
    version = "0.2.0",
    about = "Zebvix Chain full node",
    long_about = None
)]
struct Cli {
    /// Network preset. Either `mainnet` (default) or `testnet`.
    #[arg(long, default_value = "mainnet")]
    network: String,

    /// Path to the node configuration TOML file (overrides preset values).
    #[arg(long)]
    config: Option<PathBuf>,

    /// Data directory (overrides config file).
    #[arg(long)]
    data_dir: Option<PathBuf>,

    /// JSON-RPC HTTP port (overrides config file).
    #[arg(long)]
    rpc_port: Option<u16>,

    /// JSON-RPC bind address (use 127.0.0.1 behind nginx; 0.0.0.0 for direct).
    #[arg(long)]
    bind_addr: Option<String>,

    /// P2P listen port (overrides config file).
    #[arg(long)]
    p2p_port: Option<u16>,

    /// Enable validator mode (requires `VALIDATOR_KEY` env var).
    #[arg(long)]
    validator: bool,

    /// Log level (error, warn, info, debug, trace).
    #[arg(long, default_value = "info")]
    log_level: String,

    /// Print genesis block info and exit.
    #[arg(long)]
    print_genesis: bool,

    /// **DANGEROUS** — bypass the strict genesis / chain_id fail-fast at
    /// startup. By default the node refuses to boot if the on-disk genesis
    /// or stored chain_id differs from the current config (S4-B3). Use
    /// this flag only for local recovery work; it disables a fork-safety
    /// guarantee. Production deployments must NOT keep this set.
    #[arg(long)]
    allow_chain_mismatch: bool,

    /// Task #14: acknowledge the mainnet readiness predicate has
    /// `Unknown` checks (snapshot manifest binding, trie pruner wiring).
    /// `Fail` checks ALWAYS block boot regardless of this flag — they
    /// indicate a code regression. This flag is intended as a 30-day
    /// post-removal sanity gate against accidental mainnet boots while
    /// pending tasks #1 and #11 land. Required only on mainnet
    /// (chain 8989); ignored on testnet/devnet.
    #[arg(long)]
    accept_mainnet_readiness: bool,
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            error!(error = %e, "zbx-node fatal error");
            eprintln!("zbx-node: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new(&cli.log_level)),
        )
        .init();

    // P1-PROD: Global panic hook — log structured panic info via tracing before
    // the process exits. Without this, panics produce an unstructured stderr
    // message that is invisible in log-aggregation systems. This hook runs on
    // every thread (tokio worker threads included); the default backtrace is
    // still printed to stderr by Rust's panic runtime after the hook returns.
    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "unknown location".to_string());
        let msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "unknown panic payload".to_string()
        };
        tracing::error!(
            location = %location,
            message  = %msg,
            "PANIC: thread panicked — this process is now aborting; \
             check backtrace below and fix the root cause immediately"
        );
    }));

    let network = Network::parse(&cli.network).map_err(|e| format!("--network: {e}"))?;

    info!(
        version = env!("CARGO_PKG_VERSION"),
        network = ?network,
        chain_id = network.chain_id(),
        "Zebvix Node starting"
    );

    // Build base config from network preset, optionally overlaying TOML file
    let mut cfg = if let Some(path) = cli.config.as_ref() {
        if path.exists() {
            NodeConfig::from_file(path)?
        } else {
            return Err(format!("--config path does not exist: {}", path.display()).into());
        }
    } else {
        NodeConfig::for_network(network)
    };

    // Ensure chain id matches network preset (a TOML can override it intentionally).
    if cfg.chain.chain_id != network.chain_id() && cli.config.is_none() {
        cfg.chain.chain_id = network.chain_id();
    }
    // SEC-2026-05-09 (N2): when an explicit --config TOML is loaded, refuse
    // to silently honor a chain_id that disagrees with --network. This used
    // to be allowed (the TOML always won), which made it trivially easy to
    // misconfigure a mainnet node onto the testnet chain ID and start
    // signing forks. Operators must now either (a) align the TOML with
    // --network, or (b) opt in via --allow-chain-mismatch.
    if cli.config.is_some()
        && cfg.chain.chain_id != network.chain_id()
        && !cli.allow_chain_mismatch
    {
        return Err(format!(
            "N2: chain_id mismatch between --network {:?} (chain_id={}) and \
             config file (chain_id={}). This is a fork-safety guard. Either \
             align the config or pass --allow-chain-mismatch (DANGEROUS).",
            network, network.chain_id(), cfg.chain.chain_id
        ).into());
    }

    // CLI overrides
    if let Some(dir) = cli.data_dir {
        cfg.storage.data_dir = dir;
    }
    if let Some(port) = cli.rpc_port {
        cfg.rpc.http_port = port;
    }
    if let Some(addr) = cli.bind_addr {
        cfg.rpc.bind_addr = addr;
    }
    if let Some(port) = cli.p2p_port {
        cfg.network.listen_port = port;
    }
    if cli.validator {
        cfg.chain.is_validator = true;
    }

    // OPERATOR-04: Refuse to start a mainnet validator node with an empty
    // validator key.  An empty key means the node would silently participate
    // in consensus with a zeroed BLS key, producing unverifiable signatures
    // and getting slashed.  We hard-fail here to surface the misconfiguration
    // before any state is written.
    if cfg.chain.is_validator && network == Network::Mainnet {
        let env_key_present = std::env::var("VALIDATOR_KEY")
            .ok()
            .filter(|k| !k.is_empty())
            .is_some();
        let cfg_key_present = cfg.chain.validator_key
            .as_deref()
            .map(|k| !k.is_empty())
            .unwrap_or(false);
        if !env_key_present && !cfg_key_present {
            error!(
                "OPERATOR-04: --validator flag set on mainnet but no validator key found. \
                 Set the VALIDATOR_KEY environment variable to the hex-encoded BLS private key \
                 produced by `zbx-keygen generate`. \
                 See docs/VALIDATOR_GUIDE.md for the full setup procedure."
            );
            return Err("validator key required for mainnet validator mode".into());
        }
    }

    // Task #4: install the EIP-4844 KZG trusted setup before any
    // execution can run. Mainnet (chain 8989) HARD-FAILS without it;
    // testnet/devnet (chain 8990) downgrades to a warning and leaves
    // precompile 0x0B fail-closed (every blob-verifier contract reverts
    // until an operator drops a setup file in place).
    if let Some(setup_path) = cfg.chain.kzg_trusted_setup_path.as_ref() {
        match zbx_crypto::kzg::load_trusted_setup(setup_path) {
            Ok(s) => {
                let installed = zbx_crypto::kzg::init_global_kzg_settings(s);
                info!(
                    path = %setup_path.display(),
                    installed = installed,
                    "Task #4: KZG trusted setup loaded for precompile 0x0B"
                );
            }
            Err(e) if network == Network::Mainnet => {
                return Err(format!(
                    "Task #4: mainnet (chain 8989) refuses to boot — KZG trusted setup at {} \
                     could not be loaded: {}. Drop the official Ethereum KZG ceremony output at \
                     this path (https://github.com/ethereum/kzg-ceremony) and retry.",
                    setup_path.display(),
                    e
                )
                .into());
            }
            Err(e) => {
                tracing::warn!(
                    path  = %setup_path.display(),
                    error = %e,
                    "Task #4: KZG trusted setup unavailable on testnet/devnet — \
                     precompile 0x0B will fail-closed until an operator installs a setup file."
                );
            }
        }
    } else if network == Network::Mainnet {
        return Err(
            "Task #4: mainnet (chain 8989) requires `chain.kzg_trusted_setup_path` to point \
             at the official Ethereum KZG ceremony output before boot. Set it in your config TOML."
                .into(),
        );
    } else {
        tracing::warn!(
            "Task #4: no chain.kzg_trusted_setup_path configured — precompile 0x0B will \
             fail-closed on this testnet/devnet node."
        );
    }

    // Task #7: assert ZbxVaultRegistry.sol is deployed at the canonical
    // 0x..5455 address in genesis. Mainnet hard-fails on missing
    // bytecode regardless of `require_vault_genesis`; testnet/devnet
    // honours the flag (defaults to false → warning-only).
    //
    // CRITICAL: this MUST validate the SAME genesis source `ZbxNode::new`
    // will load — mirrors the `chain.genesis_file` precedence in
    // `node.rs` so an operator's custom genesis JSON is honoured. Audit
    // S4-B8 / Task-#7-rev-2: previously this checked only the preset
    // allocs and would block valid nodes whose registry lived in a
    // custom genesis file.
    {
        let genesis_cfg = if cfg.chain.genesis_file.exists()
            && cfg.chain.genesis_file != PathBuf::from("genesis.json")
        {
            info!(
                path = %cfg.chain.genesis_file.display(),
                "Task #7: validating vault genesis against custom genesis JSON"
            );
            GenesisConfig::from_file(&cfg.chain.genesis_file)
                .map_err(|e| format!("Task #7 vault-genesis check: {e}"))?
        } else {
            GenesisConfig::for_network(network)?
        };
        let alloc_iter = genesis_cfg.alloc.iter().map(|a| {
            let addr_hex = format!("0x{}", hex::encode(a.address.as_bytes()));
            (addr_hex, a.code.clone())
        });
        match crate::config::assert_vault_genesis_or_warn(
            &cfg.chain,
            network == Network::Mainnet,
            alloc_iter,
            |m| tracing::warn!("{}", m),
        ) {
            Ok(true) => info!(
                "Task #7: ZbxVaultRegistry.sol verified at canonical 0x..5455 in genesis"
            ),
            Ok(false) => {} // warning already emitted
            Err(e) => return Err(e.into()),
        }
    }

    // Task #12 (SEC-2026-05-09 keystore VPS-hardening): walk the
    // node's data directory and tighten any keyfile that was written
    // before the secure_write migration, or by an unrelated tool with
    // a permissive umask. Each tightening logs a `warn!` so operators
    // see exactly which file was leaky. Failures are non-fatal — we
    // do not want to brick a node startup over a stat() error on a
    // single auxiliary file — but they are logged.
    {
        let data_dir = &cfg.storage.data_dir;
        match zbx_keystore::tighten_dir(data_dir) {
            Ok(0) => {
                info!(
                    data_dir = %data_dir.display(),
                    "Task #12: keystore-perm scan complete — no loose files found"
                );
            }
            Ok(n) => {
                tracing::warn!(
                    data_dir = %data_dir.display(),
                    tightened = n,
                    "Task #12: tightened {} loose keystore file(s) at startup — \
                     audit who else has been writing to this directory",
                    n
                );
            }
            Err(e) => {
                tracing::warn!(
                    data_dir = %data_dir.display(),
                    error = %e,
                    "Task #12: keystore-perm scan failed — continuing boot but \
                     operators must verify <data_dir>/*.key are mode 0600"
                );
            }
        }
        let keys_dir = data_dir.join("keys");
        if keys_dir.exists() {
            if let Err(e) = zbx_keystore::tighten_dir(&keys_dir) {
                tracing::warn!(
                    keys_dir = %keys_dir.display(),
                    error = %e,
                    "Task #12: keys/ subdirectory scan failed"
                );
            }
        }
    }

    // Task #14: mainnet readiness predicate. Replaces the Pass-12
    // `assert_not_mainnet_*` panic guards (already removed in Pass-17/18)
    // with a positive, structured check that PoP / precompiles /
    // snapshot binding / pruner wiring are all live before consensus
    // can start. `Fail` gaps always block; `Unknown` gaps block unless
    // `--accept-mainnet-readiness` is passed (30-day grace gate).
    if network == Network::Mainnet {
        // Pass-19 architect-review: thread runtime config into the
        // readiness predicate so check #4 fails when the pruner is
        // disabled on mainnet without explicit archive-mode opt-in.
        let ready_ctx = readiness::ReadinessContext {
            pruner_enabled: cfg.storage.pruner.enabled,
            archive_mode:   cfg.storage.pruner.archive_mode,
        };
        match readiness::verify_mainnet_ready(ready_ctx) {
            Ok(()) => {
                info!("Task #14: mainnet readiness check PASSED — all 4 gates green");
            }
            Err(gaps) => {
                let report = readiness::format_gaps(&gaps);
                if readiness::gaps_block_boot(&gaps, cli.accept_mainnet_readiness) {
                    error!("{}", report);
                    return Err(
                        "Task #14: mainnet readiness check failed — refusing to boot. \
                         See report above. Fix the failing checks or, for non-Fail \
                         gaps only, pass --accept-mainnet-readiness.".into(),
                    );
                } else {
                    tracing::warn!(
                        "{}\nProceeding because --accept-mainnet-readiness was passed. \
                         This is a temporary grace gate — fix the Unknown gaps before \
                         the 30-day window expires.",
                        report,
                    );
                }
            }
        }
    }

    // Task #22: snapshot manifest verification at the fast-sync import
    // boundary. When `<data_dir>/snapshot.manifest.bin` is present, we
    // verify it BEFORE `ZbxNode::new` so a stale or unauthorised
    // manifest aborts boot before any state is touched. The typed
    // `ImportMode::for_live_chain` gate refuses to construct a
    // checkpoint-less mode for any chain id that matches a live
    // network — so the new freshness binding cannot be skipped on
    // mainnet/testnet by accident.
    {
        let chain_id = cfg.chain.chain_id;
        // Parse the trusted checkpoint pin (if any).
        let parsed_ckpt = match cfg.chain.trusted_snapshot_checkpoint.as_ref() {
            Some(c) => match snapshot_import::parse_checkpoint_hash(&c.hash) {
                Ok(h) => Some(snapshot_import::TrustedCheckpoint::from_chain_config(c.height, h)),
                Err(e) => return Err(format!("Task #22: trusted_snapshot_checkpoint.hash: {e}").into()),
            },
            None => None,
        };
        // Parse the allowed-producer set (if any).
        let mut allowed = Vec::with_capacity(cfg.chain.snapshot_allowed_producers.len());
        for (i, hex_pk) in cfg.chain.snapshot_allowed_producers.iter().enumerate() {
            allowed.push(snapshot_import::parse_allowed_producer(i, hex_pk).map_err(|e| format!("Task #22: snapshot_allowed_producers[{i}]: {e}"))?);
        }
        // The mode-vs-checkpoint decision happens *atomically with the
        // file read* inside `maybe_import_snapshot`. There is no
        // pre-check + later-verify split here, so a manifest file
        // appearing between two probes cannot reach the verifier in
        // tooling mode on a live chain. The decision table is
        // documented on `maybe_import_snapshot`.
        match snapshot_import::maybe_import_snapshot(
            &cfg.storage.data_dir,
            chain_id,
            &allowed,
            parsed_ckpt,
        ) {
            Ok(None) => {} // no manifest present — common case
            Ok(Some((h, root))) => {
                info!(
                    height = h,
                    state_root = %hex::encode(root.as_bytes()),
                    "Task #22: snapshot manifest verified at import boundary — \
                     fast-sync may consume the bundle"
                );
            }
            Err(e) => {
                return Err(format!(
                    "Task #22: snapshot manifest verification FAILED at import \
                     boundary — refusing to boot. {e}"
                )
                .into());
            }
        }
    }

    if cli.print_genesis {
        let genesis_cfg = GenesisConfig::for_network(network)?;
        let (block, accounts) = genesis_cfg.build_genesis_block();
        println!("Network            : {network:?}");
        println!("Chain ID           : {}", genesis_cfg.chain_id);
        println!("Genesis hash       : 0x{}", hex::encode(block.hash()));
        println!("Genesis timestamp  : {}", block.header.timestamp);
        println!("Genesis allocs     : {} accounts", accounts.len());
        println!("Initial validators : {}", genesis_cfg.validators.len());
        return Ok(());
    }

    let node = ZbxNode::new(cfg, cli.allow_chain_mismatch)?;
    node.run().await?;
    Ok(())
}
