//! # zbxctl — Zebvix Chain CLI
//!
//! The canonical command-line tool for the ZBX chain (Chain ID 8989).
//!
//! ## Usage
//! ```bash
//! zbxctl --help
//! zbxctl --version
//! zbxctl --rpc-url https://rpc.zbx.network --chain-id 8989 <COMMAND>
//! ```
//!
//! ## Subcommands
//! | Command     | Description                                |
//! |-------------|--------------------------------------------|
//! | wallet      | Key management (new, import, export, sign) |
//! | contract    | Deploy, call, send to smart contracts      |
//! | stake       | Validator staking and delegation           |
//! | governance  | Proposals and voting                       |
//! | defi        | Swap tokens, add/remove liquidity          |
//!
//! ## Config
//! Settings are loaded (in priority order) from:
//! 1. CLI flags
//! 2. Environment variables (`ZBX_RPC_URL`, `ZBX_CHAIN_ID`, `ZBX_KEYSTORE`)
//! 3. `.env` in current directory (via dotenv)
//!
//! ## Safety posture (Session 4 audit closures)
//! The CLI deliberately refuses to perform on-chain mutations until the user
//! has acknowledged a per-command preflight summary. See `safety.rs` for the
//! confirmation policy and `config.rs` for the RPC TLS / keystore policy.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod config;
mod output;
mod safety;
mod rpc;

mod wallet;
mod contract;
mod defi;
mod governance;
mod stake;

#[derive(Parser, Debug)]
#[command(
    name    = "zbxctl",
    version = env!("CARGO_PKG_VERSION"),
    about   = "Zebvix Chain CLI — interact with ZBX chain (Chain ID 8989)",
    long_about = None,
)]
pub struct Cli {
    /// JSON-RPC endpoint URL (must be `https://` for non-localhost endpoints
    /// unless `--allow-insecure-rpc` is also passed).
    #[arg(long, env = "ZBX_RPC_URL", default_value = "http://localhost:8545",
          global = true)]
    pub rpc_url: String,

    /// Chain ID for signing (ZBX mainnet = 8989).
    #[arg(long, env = "ZBX_CHAIN_ID", default_value = "8989", global = true)]
    pub chain_id: u64,

    /// Path to encrypted keystore file (Ethereum v3 JSON).
    #[arg(long, env = "ZBX_KEYSTORE", global = true)]
    pub keystore: Option<PathBuf>,

    // ─── Secret-handling policy (T210) ─────────────────────────────────────
    /// Read the keystore password from STDIN (one line, newline-stripped).
    /// Use this for scripted invocations: `printf '%s\n' "$PW" | zbxctl ...`.
    #[arg(long, global = true, conflicts_with = "password_file")]
    pub password_stdin: bool,

    /// Read the keystore password from a file (single line, newline-stripped).
    /// The file should be readable only by the current user (mode 0400/0600).
    #[arg(long, global = true)]
    pub password_file: Option<PathBuf>,

    // ─── Network policy (T214) ─────────────────────────────────────────────
    /// Allow plain `http://` against non-localhost RPC endpoints. Off by
    /// default — requests over plain HTTP can be observed and rewritten by
    /// any network on the path.
    #[arg(long, global = true)]
    pub allow_insecure_rpc: bool,

    // ─── Confirmation policy (T211) ────────────────────────────────────────
    /// Skip the interactive `[y/N]` confirmation for state-changing actions.
    /// Required for unattended/CI runs. The preflight summary is still
    /// printed to STDERR.
    #[arg(long, global = true)]
    pub yes: bool,

    /// Verbose logging (RUST_LOG=debug equivalent).
    #[arg(short, long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Key and wallet management
    Wallet(wallet::WalletCmd),
    /// Deploy and interact with smart contracts
    Contract(contract::ContractCmd),
    /// Validator staking and delegation
    Stake(stake::StakeCmd),
    /// Governance proposals and voting
    Governance(governance::GovernanceCmd),
    /// DeFi — swaps, liquidity, tokens
    Defi(defi::DefiCmd),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load `.env` if present (silently ignore missing).
    let _ = dotenv::dotenv();

    let cli = Cli::parse();
    output::init_tracing(cli.verbose);

    let policy = config::CliPolicy {
        password_stdin: cli.password_stdin,
        password_file: cli.password_file.clone(),
        allow_insecure_rpc: cli.allow_insecure_rpc,
        yes: cli.yes,
    };
    let ctx = config::Context::from_parts(
        cli.rpc_url.clone(),
        cli.chain_id,
        cli.keystore.clone(),
        policy,
    )?;

    match cli.command {
        Commands::Wallet(cmd)     => wallet::run(cmd, &ctx).await,
        Commands::Contract(cmd)   => contract::run(cmd, &ctx).await,
        Commands::Stake(cmd)      => stake::run(cmd, &ctx).await,
        Commands::Governance(cmd) => governance::run(cmd, &ctx).await,
        Commands::Defi(cmd)       => defi::run(cmd, &ctx).await,
    }
}
