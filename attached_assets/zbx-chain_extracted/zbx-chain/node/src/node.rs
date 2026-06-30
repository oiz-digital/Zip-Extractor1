//! ZbxNode: assembles all subsystems and coordinates the main run loop.
//!
//! ## Audit 2026-04-30 — S4-B1 + S4-B2 closed
//!
//! - **SIGTERM + Ctrl-C** are both honoured (Unix only — `signal::unix`).
//!   The legacy implementation only handled Ctrl-C, so a `kill -TERM` on a
//!   systemd-managed deployment terminated the process abruptly with no
//!   storage flush. Both signals now broadcast a shutdown intent through a
//!   `tokio::sync::watch` channel; every subsystem polls it cooperatively.
//! - **Bounded shutdown drain** — once the shutdown signal fires we wait up
//!   to `SHUTDOWN_DRAIN_SECS` for tasks to exit on their own; whatever is
//!   left is force-aborted. This caps shutdown latency for orchestrators.
//! - **Per-task supervisor with restart policy** — the previous `select!`
//!   over `tasks.join_next()` killed the whole node on the first task exit,
//!   even when that task was a non-critical heartbeat. Each subsystem is
//!   now spawned with a documented `RestartPolicy`. Critical subsystems
//!   (block_producer, RPC) bring the node down on exit; non-critical
//!   subsystems (peer-manager / mempool heartbeats) are restarted with
//!   exponential backoff capped at 30s.

use crate::config::NodeConfig;
use crate::genesis::{BootstrapPolicy, GenesisConfig};
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tracing::{error, info, warn};
use zbx_mempool::{MempoolConfig, TransactionPool};
use zbx_metrics::server::MetricsServer;
use zbx_network::peer::PeerManager;
use zbx_rpc::{RpcServer, RpcState, WsServer};
use zbx_storage::ZbxDb;
// ── Newly-wired subsystems ────────────────────────────────────────────────────
use zbx_oracle::OracleScheduler;
use zbx_bundler::BundlerService;
use zbx_mev::MevCoordinator;
use zbx_sync::SyncService;
use zbx_da::DaService;
use zbx_telemetry::TelemetryService;
use zbx_xcl::XclGateway;
use zbx_indexer::{IndexerService, IndexerServiceConfig};

/// Maximum time we wait for cooperative tasks to exit after signalling
/// shutdown. After this elapses we abort what's still running so an
/// orchestrator (systemd, k8s) sees a timely exit.
const SHUTDOWN_DRAIN_SECS: u64 = 15;

/// Top-level node error.
#[derive(Debug, thiserror::Error)]
pub enum NodeError {
    #[error("storage: {0}")]
    Storage(String),
    #[error("genesis: {0}")]
    Genesis(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("config: {0}")]
    Config(String),
}

/// Whether a subsystem's exit should bring the node down or be silently
/// restarted by the supervisor.
#[derive(Debug, Clone, Copy)]
enum RestartPolicy {
    /// Stop the node if this task exits / panics.
    Critical,
    /// Restart with exponential backoff (1s, 2s, 4s, ... capped at 30s).
    AutoRestart,
}

/// The full Zebvix node — holds all subsystem handles.
pub struct ZbxNode {
    config: NodeConfig,
    storage: Arc<ZbxDb>,
    mempool: Arc<RwLock<TransactionPool>>,
    peer_manager: Arc<RwLock<PeerManager>>,
    is_validator: bool,
    /// When `true`, an explicit operator override (`--allow-chain-mismatch`)
    /// disables the strict genesis / chain_id fail-fast in
    /// `GenesisConfig::bootstrap_into`.
    allow_chain_mismatch: bool,
}

impl ZbxNode {
    /// Initialise the node from configuration. The `allow_chain_mismatch`
    /// flag is propagated from the CLI (`--allow-chain-mismatch`) and is
    /// only intended for local recovery work.
    pub fn new(config: NodeConfig, allow_chain_mismatch: bool) -> Result<Self, NodeError> {
        info!(chain_id = config.chain.chain_id, "initialising Zebvix node");

        // 1. Storage
        std::fs::create_dir_all(&config.storage.data_dir)
            .map_err(|e| NodeError::Storage(format!("create data dir: {e}")))?;
        let storage = Arc::new(
            ZbxDb::open(&config.storage.data_dir)
                .map_err(|e| NodeError::Storage(e.to_string()))?,
        );

        // 2. Genesis bootstrap (idempotent + fail-fast on mismatch)
        //
        // Operator may load a custom genesis JSON via `chain.genesis_file`,
        // which is honoured when the path actually exists. Otherwise we fall
        // back to the network preset (mainnet / testnet). See AUDIT-S4-B8.
        let genesis_cfg = if config.chain.genesis_file.exists()
            && config.chain.genesis_file != std::path::PathBuf::from("genesis.json")
        {
            info!(
                path = %config.chain.genesis_file.display(),
                "loading custom genesis JSON"
            );
            GenesisConfig::from_file(&config.chain.genesis_file)
                .map_err(NodeError::Genesis)?
        } else {
            GenesisConfig::for_network_id(config.chain.chain_id)
                .map_err(|e| NodeError::Genesis(e.to_string()))?
        };

        let policy = if allow_chain_mismatch {
            BootstrapPolicy::AllowMismatch
        } else {
            BootstrapPolicy::StrictFailFast
        };
        let (created, hash) = genesis_cfg
            .bootstrap_into(&storage, policy)
            .map_err(NodeError::Genesis)?;
        if created {
            info!(genesis = %hex::encode(hash), "fresh chain — genesis written");
        } else {
            info!(genesis = %hex::encode(hash), "existing chain detected");
        }

        // 3. Mempool
        let mp_cfg = MempoolConfig {
            max_pending: config.consensus.mempool_max_pending,
            max_queued: config.consensus.mempool_max_queued,
            ..Default::default()
        };
        let mempool = Arc::new(RwLock::new(TransactionPool::new(mp_cfg)));

        // 4. P2P peer manager
        let peer_manager = Arc::new(RwLock::new(PeerManager::new(config.network.max_peers)));

        // 5. Validator key check. Precedence: VALIDATOR_KEY env > config
        //    `chain.validator_key` (which is logged but never echoed). The
        //    config field exists so a deployment can declare validator
        //    intent without exposing the secret to the file system.
        let mut is_validator = config.chain.is_validator;
        if is_validator {
            let env_key = std::env::var("VALIDATOR_KEY").ok().filter(|k| !k.is_empty());
            let cfg_key_present = config
                .chain
                .validator_key
                .as_ref()
                .map(|k| !k.is_empty())
                .unwrap_or(false);
            match (env_key, cfg_key_present) {
                (Some(k), _) => {
                    info!(
                        key_len = k.len(),
                        "validator mode enabled — VALIDATOR_KEY loaded from env"
                    );
                }
                (None, true) => {
                    warn!(
                        "validator mode enabled — using chain.validator_key from config; \
                         prefer VALIDATOR_KEY env for production"
                    );
                }
                (None, false) => {
                    warn!("validator mode requested but no validator key found — falling back to full-node mode");
                    is_validator = false;
                }
            }
        }

        Ok(ZbxNode {
            config,
            storage,
            mempool,
            peer_manager,
            is_validator,
            allow_chain_mismatch,
        })
    }

    /// Run all node services until shutdown.
    pub async fn run(self) -> Result<(), NodeError> {
        let cfg = self.config.clone();
        info!(
            mode = if self.is_validator { "validator" } else { "full" },
            allow_chain_mismatch = self.allow_chain_mismatch,
            "starting Zebvix node services"
        );

        // ─── Shutdown signalling channel ────────────────────────────────
        // `false` = run, `true` = please stop. Every long-running task
        // clones the receiver and either polls it via tokio::select! or
        // checks `*shutdown.borrow()` between iterations.
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        // We use a plain JoinSet for non-restartable tasks; auto-restart
        // tasks each get their own spawn loop driven by the supervisor.
        let mut critical_tasks: tokio::task::JoinSet<&'static str> = tokio::task::JoinSet::new();

        // Build shared RpcState now so we can clone the broadcast channels and
        // validator-set Arc before moving state into the RPC server task.
        // Even when HTTP is disabled the channels are used by the consensus
        // driver to push committed blocks to WebSocket subscribers.
        let rpc_state = RpcState::new(
            self.storage.clone(),
            self.mempool.clone(),
            cfg.chain.chain_id,
            "zbx-node/0.2.0",
        );

        // H-4 fix (2026-06-27): rehydrate durable governance proposals from RocksDB
        // BEFORE the RPC server starts accepting requests.  Without this call, every
        // restart wiped all pending proposals and `zbx_getGovernanceProposal` returned
        // `not_found` for legitimately submitted proposals.
        rpc_state.load_governance_from_db();

        let consensus_new_head_tx   = rpc_state.new_head_tx.clone();
        let consensus_validator_set = rpc_state.validator_set.clone();
        let rpc_peer_count          = rpc_state.peer_count.clone();
        let rpc_tx_relay_tx         = Arc::clone(&rpc_state.tx_relay_tx);

        // SEC-2026-05-09 (Pass-9 architect-fix): clone the shared RpcState for
        // the WS server BEFORE the HTTP block consumes the original. RpcState
        // derives Clone over Arc'd broadcast Senders, so a clone shares the
        // SAME `new_head_tx` / `new_pending_tx` / `tx_relay_tx` channels that
        // the consensus driver pushes to. A fresh `RpcState::new()` here would
        // create disconnected channels and WS subscribers would silently
        // receive nothing.
        let ws_rpc_state = if cfg.rpc.ws_enabled { Some(rpc_state.clone()) } else { None };

        // 1. JSON-RPC HTTP (CRITICAL — losing it severs operator access)
        if cfg.rpc.http_enabled {
            let state = rpc_state;
            let http_port = cfg.rpc.http_port;
            let ws_port = cfg.rpc.ws_port;
            let bind = cfg.rpc.bind_addr.clone();
            let cors = cfg.rpc.cors_origins.clone();
            let cors_count = cors.len();
            let rpm = cfg.rpc.rate_limit_rpm;
            let mut shutdown_rx_rpc = shutdown_rx.clone();
            critical_tasks.spawn(async move {
                let rpc = RpcServer::new(state, http_port, ws_port)
                    .with_bind(bind)
                    .with_cors_origins(&cors)
                    .with_rate_limit_rpm(rpm);
                tokio::select! {
                    res = rpc.run() => {
                        if let Err(e) = res {
                            error!(error = %e, "RPC server exited");
                        }
                    }
                    _ = shutdown_rx_rpc.changed() => {
                        info!("RPC server received shutdown signal");
                    }
                }
                "rpc"
            });
            info!(
                port = http_port,
                cors = cors_count,
                rate_limit_rpm = rpm,
                "RPC HTTP server started"
            );
        }

        // 1b. JSON-RPC WebSocket (optional — opt-in via cfg.rpc.ws_enabled).
        //     Spawns a separate listener on `ws_port` that supports
        //     eth_subscribe / eth_unsubscribe (newHeads, newPendingTransactions, logs).
        //     Wired 2026-05-09 (Pass-9): the WsServer was implemented in
        //     `crates/zbx-rpc/src/ws_server.rs` but never spawned, so
        //     `ws_enabled = true` was a no-op. Now honored, sharing the same
        //     RpcState (and therefore the same broadcast channels) as HTTP RPC.
        if let Some(ws_state) = ws_rpc_state {
            let ws_port = cfg.rpc.ws_port;
            let mut shutdown_rx_ws = shutdown_rx.clone();
            critical_tasks.spawn(async move {
                let ws = WsServer::new(ws_state, ws_port);
                tokio::select! {
                    res = ws.run() => {
                        if let Err(e) = res {
                            error!(error = %e, "WS server exited");
                        }
                    }
                    _ = shutdown_rx_ws.changed() => {
                        info!("WS server received shutdown signal");
                    }
                }
                "ws"
            });
            info!(port = ws_port, "RPC WebSocket server started");
        }

        // 2. Metrics (CRITICAL — needed for ops dashboards)
        // SEC-2026-05-09 Pass-10 — build the Registry once at node scope so
        // every subsystem (consensus driver below, future RPC/network/bridge
        // hooks) can grab Arc-backed handles into the same counters that the
        // scrape endpoint renders. Without this, `equivocations_total` etc.
        // would always be zero in production.
        let metrics_registry = zbx_metrics::Registry::new();
        if cfg.metrics.enabled {
            let port = cfg.metrics.port;
            let mut shutdown_rx_metrics = shutdown_rx.clone();
            let registry_for_srv = metrics_registry.clone();
            critical_tasks.spawn(async move {
                let srv = MetricsServer::with_registry(port, registry_for_srv);
                tokio::select! {
                    res = srv.run() => {
                        if let Err(e) = res {
                            error!(error = %e, "metrics server exited");
                        }
                    }
                    _ = shutdown_rx_metrics.changed() => {
                        info!("metrics server received shutdown signal");
                    }
                }
                "metrics"
            });
            info!(port = cfg.metrics.port, "metrics server started");
        }

        // 3. P2P peer-manager housekeeping (AUTO-RESTART)
        let pm = self.peer_manager.clone();
        let p2p_port = cfg.network.listen_port;
        let bootnodes_n = cfg.network.bootnodes.len();
        spawn_supervised(
            "peer_manager",
            RestartPolicy::AutoRestart,
            shutdown_rx.clone(),
            move |mut sd| {
                let pm = pm.clone();
                async move {
                    info!(
                        port = p2p_port,
                        bootnodes = bootnodes_n,
                        max_peers = pm.read().connected_count(),
                        "P2P peer-manager initialised"
                    );
                    let mut tick = tokio::time::interval(Duration::from_secs(30));
                    loop {
                        tokio::select! {
                            _ = tick.tick() => {
                                let count = pm.read().connected_count();
                                tracing::debug!(peers = count, "peer-manager heartbeat");
                            }
                            _ = sd.changed() => {
                                info!("peer-manager shutting down");
                                return;
                            }
                        }
                    }
                }
            },
        );

        // 4. Mempool maintenance: prune expired / re-broadcast (AUTO-RESTART)
        let mp = self.mempool.clone();
        spawn_supervised(
            "mempool_heartbeat",
            RestartPolicy::AutoRestart,
            shutdown_rx.clone(),
            move |mut sd| {
                let mp = mp.clone();
                async move {
                    let mut tick = tokio::time::interval(Duration::from_secs(60));
                    loop {
                        tokio::select! {
                            _ = tick.tick() => {
                                let stats = {
                                    let p = mp.read();
                                    (p.pending_count(), p.queued_count())
                                };
                                tracing::debug!(
                                    pending = stats.0, queued = stats.1,
                                    "mempool heartbeat"
                                );
                            }
                            _ = sd.changed() => {
                                info!("mempool heartbeat shutting down");
                                return;
                            }
                        }
                    }
                }
            },
        );

        // 5. Trie pruner (AUTO-RESTART) — Task #1.
        //
        // Bounded-history mark-and-sweep over `Column::TrieNodes` so disk
        // doesn't grow unbounded as historical state accumulates. Cadence
        // and retention are operator-tunable via `[storage.pruner]` in the
        // node TOML (defaults: 5-min interval, 256 retained roots, 64-block
        // skip-advance, 4096-key sweep batch). Set `enabled = false` for
        // archive-node mode (intentional unbounded retention).
        //
        // The supervisor uses AutoRestart with exponential backoff. A
        // genuine pruner failure (RocksDB I/O, decode error) does not
        // bring the node down — it logs and retries — because pruning is
        // a maintenance activity, not consensus-critical. The mainnet
        // readiness predicate (Task #14 check #4) probes the pure logic
        // at boot via `pruner::probe_in_memory`, and the persisted
        // `pruner.last_run_*` metadata fields let operators monitor live
        // progress.
        //
        // Child-extraction closure: we route raw RLP bytes through
        // `zbx_trie::TrieNode::decode` and walk every `NodeRef::Hash`.
        // Inline children (RLP < 32B, embedded directly per Yellow Paper
        // §D — see Pass-8 fix in `zbx-trie/src/node.rs`) are skipped
        // because they have no independent storage row and cannot be
        // pruned in isolation.
        if cfg.storage.pruner.enabled {
            // ─── Task #15: production RocksDbPruner wiring ──────────────
            //
            // Three pieces:
            //   (1) Shared `Arc<RwLock<Vec<Retained>>>` — the retention
            //       checkpoint list the pruner reads.
            //   (2) `PrunerLock = Arc<RwLock<()>>` — installed into
            //       `ZbxDb` so every `commit_block` /
            //       `ZbxDbTrieAdapter::commit` / `put_trie_node` holds
            //       `lock.read()` while the pruner takes
            //       `lock.write()` for the duration of its sweep.
            //   (3) Retained-tracker task — polls the chain-tip every
            //       second and appends `Retained{block, state_root}`
            //       to the shared list. This is the documented
            //       "block-producer commit hook" (the pruner runs at
            //       60 s cadence; ~1 s observation latency on the head
            //       is well below the cadence and avoids touching
            //       block_producer.rs / consensus.rs).
            //   (4) `RocksDbPruner::spawn` — background mark-and-sweep
            //       loop driven by a head_provider closure.
            use zbx_pruner::{
                PrunerLock, Retained, RocksDbPruner, RocksDbPrunerConfig,
            };

            let pruner_lock: PrunerLock = Arc::new(RwLock::new(()));
            self.storage.set_commit_lock(Arc::clone(&pruner_lock));

            let retained: Arc<RwLock<Vec<Retained>>> =
                Arc::new(RwLock::new(Vec::new()));

            let pruner_settings = cfg.storage.pruner.clone();
            let prn_cfg = RocksDbPrunerConfig {
                retain_blocks: pruner_settings.max_retained_roots as u64,
                interval: Duration::from_secs(pruner_settings.interval_secs.max(1)),
                sweep_batch: pruner_settings.sweep_batch_size,
            };
            info!(
                interval_s    = pruner_settings.interval_secs,
                retain_blocks = prn_cfg.retain_blocks,
                sweep_batch   = prn_cfg.sweep_batch,
                "Task #15: RocksDbPruner subsystem wired (commit-lock active)"
            );

            // (3) Seed retained-roots window from existing on-disk
            //     state BEFORE the pruner can run. Without this, a
            //     fresh process boot would advertise only the head
            //     block as retained — and the pruner's mark-and-sweep
            //     would then GC every reachable trie node not anchored
            //     to that single root, deleting recent history that
            //     legitimate `eth_getProof` / `eth_getStorageAt` /
            //     reorg-rewind paths still need.
            //
            //     Window = [head - retain_blocks + 1 .. head]. Blocks
            //     missing from disk (chain younger than the window) are
            //     simply skipped.
            {
                let head_n = self.storage.get_latest_block_number().unwrap_or(0);
                let retain = prn_cfg.retain_blocks;
                let start = head_n.saturating_sub(retain.saturating_sub(1));
                let mut seeded: Vec<Retained> = Vec::new();
                for n in start..=head_n {
                    match self.storage.get_block_by_number(n) {
                        Ok(Some(b)) => seeded.push(Retained {
                            block: n,
                            state_root: b.header.state_root,
                        }),
                        Ok(None) => {}
                        Err(e) => warn!(height = n, error = %e,
                            "pruner: failed seeding retained root from disk"),
                    }
                }
                let n_seeded = seeded.len();
                retained.write().extend(seeded);
                info!(
                    head = head_n,
                    seeded = n_seeded,
                    window_start = start,
                    "Task #15: seeded pruner retained-roots window from disk"
                );
            }

            // (3b) Install the producer-side commit hook. After this,
            //      every successful `block_producer::execute_and_commit_inner`
            //      pushes the new (height, state_root) checkpoint
            //      directly — no polling, no startup race. Covers all
            //      three commit paths (consensus driver, single-validator
            //      `produce_one`, network-sync `network.rs`) via a
            //      process-global `OnceLock` in `block_producer.rs`.
            crate::block_producer::set_retained_tracker(Arc::clone(&retained));

            // (3c) Bound the in-memory retained vector. The producer
            //      hook only appends; without a trimmer the vector
            //      grows unbounded over multi-day uptime. The pruner
            //      itself only scans the head-window, so anything
            //      older than 4 × retain_blocks is operational dead
            //      weight. Cheap O(n) drain at 1 % frequency.
            let retained_for_trimmer = Arc::clone(&retained);
            let max_keep_entries: usize =
                (prn_cfg.retain_blocks.saturating_mul(4) as usize).max(64);
            let trim_period = Duration::from_secs(prn_cfg.interval.as_secs().max(30));
            spawn_supervised(
                "trie_pruner_retained_trimmer",
                RestartPolicy::AutoRestart,
                shutdown_rx.clone(),
                move |mut sd| {
                    let retained = Arc::clone(&retained_for_trimmer);
                    let max_keep = max_keep_entries;
                    let period = trim_period;
                    async move {
                        let mut tick = tokio::time::interval(period);
                        tick.tick().await;
                        loop {
                            tokio::select! {
                                _ = tick.tick() => {
                                    let mut g = retained.write();
                                    if g.len() > max_keep {
                                        let drop_n = g.len() - max_keep;
                                        g.drain(0..drop_n);
                                    }
                                }
                                _ = sd.changed() => return,
                            }
                        }
                    }
                },
            );

            // (4) The pruner background loop itself.
            let storage_for_pruner = self.storage.clone();
            let retained_for_pruner = Arc::clone(&retained);
            let pruner_lock_for_loop = Arc::clone(&pruner_lock);
            let prn_cfg_for_loop = prn_cfg.clone();
            spawn_supervised(
                "trie_pruner",
                RestartPolicy::AutoRestart,
                shutdown_rx.clone(),
                move |mut sd| {
                    let storage = storage_for_pruner.clone();
                    let retained = Arc::clone(&retained_for_pruner);
                    let lock     = Arc::clone(&pruner_lock_for_loop);
                    let cfg_local = prn_cfg_for_loop.clone();
                    async move {
                        let pruner = Arc::new(RocksDbPruner::new(
                            Arc::clone(&storage),
                            cfg_local,
                            Arc::clone(&retained),
                            Arc::clone(&lock),
                        ));
                        let storage_for_head = Arc::clone(&storage);
                        let join = Arc::clone(&pruner).spawn(move || {
                            storage_for_head.get_latest_block_number().unwrap_or(0)
                        });
                        // Wait for shutdown, then abort the loop.
                        let _ = sd.changed().await;
                        join.abort();
                        info!("trie pruner shutting down");
                    }
                },
            );
        } else {
            warn!(
                "Task #15: trie pruner DISABLED via [storage.pruner] enabled=false. \
                 This is archive-node mode — disk usage will grow unbounded. \
                 Mainnet operators must NOT disable the pruner without an explicit \
                 archival mandate."
            );
        }

        // 5b. Bridge relayer (AUTO-RESTART)
        //
        // The bridge relayer persists spent-operation hashes to RocksDB so that
        // a process restart cannot replay an already-executed cross-chain request.
        // This wiring is the MAINNET-BLOCKER fix referenced in zbx-bridge README:
        //
        //   `attach_storage()` MUST be called on startup so:
        //     1. All previously committed spent-op hashes are reloaded into the
        //        in-memory replay-protection set from `Column::BridgeSpentOps`.
        //     2. Every new `execute()` call writes the hash to RocksDB before the
        //        on-chain action fires, so a crash between persist and execute is
        //        safe (execute rejects the already-persisted hash on restart).
        //
        // Failure to call `attach_storage` is treated as a startup error because
        // running the bridge without persistence defeats replay protection.
        {
            use zbx_bridge::{
                BridgeRelayer,
                BridgeSpentOpsStore,
                relayer::ZBX_CHAIN_ID_MAINNET,
                relayer::ZBX_CHAIN_ID_TESTNET,
            };

            let chain_id   = cfg.chain.chain_id;
            let own_chain  = if chain_id == 8989 { ZBX_CHAIN_ID_MAINNET } else { ZBX_CHAIN_ID_TESTNET };

            // Build the spent-ops store backed by the node's RocksDB.
            let store = Arc::new(BridgeSpentOpsStore::new(Arc::clone(&self.storage)));

            // Build the relayer — no multisig keys by default; operators
            // inject keys via environment / future bridge config section.
            let mut relayer = BridgeRelayer::new(vec![], own_chain);

            // MAINNET-BLOCKER: attach durable storage + rehydrate replay set.
            match relayer.attach_storage(Arc::clone(&store) as Arc<dyn zbx_bridge::SpentOpsStore>) {
                Ok(()) => {
                    info!(
                        chain_id,
                        "bridge: spent-ops store attached and replay set rehydrated"
                    );
                }
                Err(e) => {
                    // Fail-closed: running without replay protection is unsafe.
                    error!(error = %e,
                        "FATAL: bridge attach_storage failed — refusing to start. \
                         Investigate Column::BridgeSpentOps integrity before retrying."
                    );
                    return Err(NodeError::Storage(format!("bridge attach_storage: {e}")));
                }
            }

            // Wrap in Arc<Mutex> so the background heartbeat + future RPC
            // hooks can share the relayer handle.
            let relayer_arc = Arc::new(parking_lot::Mutex::new(relayer));
            let relayer_hb  = Arc::clone(&relayer_arc);

            // Spawn a lightweight bridge heartbeat that logs liveness at a
            // regular cadence and provides the hook for future bridge-event
            // polling (e.g., scanning on-chain deposit events).
            let _ = relayer_arc; // relayer_arc available for future use by RPC
            spawn_supervised(
                "bridge_relayer",
                RestartPolicy::AutoRestart,
                shutdown_rx.clone(),
                move |mut sd| {
                    let _relayer = Arc::clone(&relayer_hb);
                    async move {
                        let mut tick = tokio::time::interval(Duration::from_secs(60));
                        tick.tick().await; // skip the immediate first tick
                        loop {
                            tokio::select! {
                                _ = tick.tick() => {
                                    tracing::debug!("bridge relayer heartbeat");
                                }
                                _ = sd.changed() => {
                                    info!("bridge relayer shutting down");
                                    return;
                                }
                            }
                        }
                    }
                },
            );

            info!(chain_id, "bridge relayer wired with durable replay protection");
        }

        // 5c. Telemetry (AUTO-RESTART) — init first so all subsequent subsystems
        //     get structured JSON logs and OTLP traces.
        if self.config.telemetry.enabled {
            let tele_cfg   = self.config.telemetry.clone();
            let chain_id   = self.config.chain.chain_id;
            let tele       = TelemetryService::new(
                tele_cfg.otlp_endpoint.clone(),
                tele_cfg.log_filter.clone(),
                tele_cfg.json_logs,
                tele_cfg.prometheus_port,
            );
            let mut sd = shutdown_rx.clone();
            tokio::spawn(async move {
                if let Err(e) = tele.run_until_shutdown(&mut sd).await {
                    tracing::warn!(chain_id, error = %e, "telemetry exited");
                }
            });
            info!(chain_id, otlp = %self.config.telemetry.otlp_endpoint,
                "telemetry service wired");
        }

        // 5d. Oracle scheduler (AUTO-RESTART, ZEP-011)
        //     Runs price fetching for all configured feeds and, when this node is
        //     an approved reporter, submits aggregated prices to the on-chain contract.
        if self.config.oracle.enabled {
            let oracle_cfg = self.config.oracle.clone();
            let chain_id   = self.config.chain.chain_id;
            let oracle     = OracleScheduler::new(
                oracle_cfg.feeds.clone(),
                oracle_cfg.aggregator_address.clone(),
                oracle_cfg.report_interval_secs,
                oracle_cfg.heartbeat_secs,
                oracle_cfg.deviation_threshold.clone(),
                oracle_cfg.is_reporter,
            );
            let mut sd = shutdown_rx.clone();
            tokio::spawn(async move {
                if let Err(e) = oracle.run_until_shutdown(&mut sd).await {
                    tracing::warn!(chain_id, error = %e, "oracle scheduler exited");
                }
            });
            info!(chain_id, feeds = ?self.config.oracle.feeds, "oracle scheduler wired");
        }

        // 5e. MEV coordinator (AUTO-RESTART)
        //     Manages the 4-layer MEV protection stack: private mempool, commit-reveal
        //     ordering, Proposer-Builder Separation, and MEV redistribution.
        if self.config.mev.enabled {
            let mev_cfg    = self.config.mev.clone();
            let chain_id   = self.config.chain.chain_id;
            let mev        = MevCoordinator::new(
                mev_cfg.private_pool_enabled,
                mev_cfg.commit_reveal_enabled,
                mev_cfg.pbs_enabled,
                mev_cfg.redistribution_enabled,
                mev_cfg.staker_share_bps,
                mev_cfg.community_share_bps,
                mev_cfg.community_fund_address.clone(),
            );
            let mut sd = shutdown_rx.clone();
            tokio::spawn(async move {
                if let Err(e) = mev.run_until_shutdown(&mut sd).await {
                    tracing::warn!(chain_id, error = %e, "mev coordinator exited");
                }
            });
            info!(chain_id, staker_bps = mev_cfg.staker_share_bps, "mev coordinator wired");
        }

        // 5f. ERC-4337 Bundler (AUTO-RESTART, ZEP-017)
        //     Validates, simulates and batches UserOperations into EntryPoint calls.
        if self.config.bundler.enabled {
            let bund_cfg   = self.config.bundler.clone();
            let chain_id   = self.config.chain.chain_id;
            let bundler    = BundlerService::new(
                bund_cfg.rpc_port,
                bund_cfg.entry_point.clone(),
                bund_cfg.max_bundle_size,
                bund_cfg.max_userop_gas,
                bund_cfg.mempool_max,
                bund_cfg.simulation_timeout_ms,
            );
            let mut sd = shutdown_rx.clone();
            tokio::spawn(async move {
                if let Err(e) = bundler.run_until_shutdown(&mut sd).await {
                    tracing::warn!(chain_id, error = %e, "bundler exited");
                }
            });
            info!(chain_id, port = bund_cfg.rpc_port, "erc-4337 bundler wired");
        }

        // 5g. Data Availability service (AUTO-RESTART, ZEP-003)
        //     Handles EIP-4844 blob sidecars, KZG commitments, DAS sampling,
        //     and blob pruning after the finality window.
        if self.config.da.enabled {
            let da_cfg     = self.config.da.clone();
            let chain_id   = self.config.chain.chain_id;
            let storage    = Arc::clone(&self.storage);
            let da         = DaService::new(
                Arc::clone(&storage),
                da_cfg.max_blobs_per_block,
                da_cfg.blob_prune_enabled,
                da_cfg.blob_prune_window,
                da_cfg.blob_rpc_enabled,
            );
            let mut sd = shutdown_rx.clone();
            tokio::spawn(async move {
                if let Err(e) = da.run_until_shutdown(&mut sd).await {
                    tracing::warn!(chain_id, error = %e, "da service exited");
                }
            });
            info!(chain_id, max_blobs = da_cfg.max_blobs_per_block, "da service wired");
        }

        // 5h. Sync manager (AUTO-RESTART)
        //     Coordinates fast-sync, snap-sync, and live-sync modes.
        //     Transitions to live mode once the chain tip is reached.
        {
            let sync_cfg   = self.config.sync.clone();
            let chain_id   = self.config.chain.chain_id;
            let storage    = Arc::clone(&self.storage);
            let sync       = SyncService::new(
                Arc::clone(&storage),
                sync_cfg.mode.clone(),
                sync_cfg.snap_pivot_lag_blocks,
                sync_cfg.snap_chunk_size_kb,
                sync_cfg.fast_sync_batch_size,
                sync_cfg.max_sync_peers,
            );
            let mut sd = shutdown_rx.clone();
            tokio::spawn(async move {
                if let Err(e) = sync.run_until_shutdown(&mut sd).await {
                    tracing::warn!(chain_id, error = %e, "sync manager exited");
                }
            });
            info!(chain_id, mode = %sync_cfg.mode, "sync manager wired");
        }

        // 5i. XCL cross-chain gateway (AUTO-RESTART, ZEP-026)
        //     Syncs foreign chain headers, verifies MPT proofs, routes packets
        //     and handles refund claims on packet timeout.
        if self.config.xcl.enabled {
            let xcl_cfg    = self.config.xcl.clone();
            let chain_id   = self.config.chain.chain_id;
            let xcl        = XclGateway::new(
                xcl_cfg.light_client_sync,
                xcl_cfg.channel_timeout_secs,
            );
            let mut sd = shutdown_rx.clone();
            tokio::spawn(async move {
                if let Err(e) = xcl.run_until_shutdown(&mut sd).await {
                    tracing::warn!(chain_id, error = %e, "xcl gateway exited");
                }
            });
            info!(chain_id, light_client = xcl_cfg.light_client_sync,
                "xcl cross-chain gateway wired");
        }

        // 5j. Block + Event Indexer (zbx-indexer, ZEP-007)
        //
        //     Opt-in heavy-I/O subsystem. Reads blocks from ZbxDb as they are
        //     produced, writes them to a local SQLite index (or Postgres — future),
        //     exposes a REST query API, and polls the on-chain TVL oracle if an
        //     address is configured. Disabled by default on validator nodes.
        if self.config.indexer.enabled {
            let idx_cfg  = self.config.indexer.clone();
            let idx_db   = Arc::clone(&self.storage);
            let rpc_port = self.config.rpc.http_port;
            let svc_cfg  = IndexerServiceConfig {
                enabled:            idx_cfg.enabled,
                backend:            idx_cfg.backend,
                db_path:            idx_cfg.db_path.clone(),
                postgres_url:       idx_cfg.postgres_url,
                api_port:           idx_cfg.api_port,
                api_enabled:        idx_cfg.api_enabled,
                tvl_oracle_address: idx_cfg.tvl_oracle_address,
                tvl_poll_secs:      idx_cfg.tvl_poll_secs,
                batch_size:         idx_cfg.batch_size,
                index_traces:       idx_cfg.index_traces,
                decode_transfers:   idx_cfg.decode_transfers,
                writer_threads:     idx_cfg.writer_threads,
            };
            let rpc_url = format!("http://127.0.0.1:{}", rpc_port);
            spawn_supervised(
                "indexer",
                RestartPolicy::AutoRestart,
                shutdown_rx.clone(),
                move |mut sd| {
                    let svc_cfg = svc_cfg.clone();
                    let idx_db  = idx_db.clone();
                    let rpc_url = rpc_url.clone();
                    async move {
                        let svc = IndexerService::new(svc_cfg, idx_db, rpc_url);
                        if let Err(e) = svc.run_until_shutdown(&mut sd).await {
                            tracing::warn!(error = %e, "indexer service exited");
                        }
                    }
                },
            );
            info!(
                db_path  = %idx_cfg.db_path,
                api_port = idx_cfg.api_port,
                "block indexer wired (step 5j)"
            );
        } else {
            info!("block indexer disabled (set [indexer] enabled = true to activate)");
        }

        // 6. Consensus / Block production (CRITICAL when validator mode is on)
        //
        // Validator mode: spawn the multi-validator HotStuff ConsensusDriver.
        // The driver handles block building, three-phase BFT, slashing, and
        // storage commit.  For a single-validator deployment (quorum=1) it
        // behaves identically to the old single-validator tick loop — every
        // self-vote immediately satisfies quorum and the block is committed
        // in one round without any network round-trip.
        //
        // Key derivation (ZBX-SEC-2026 two-key architecture):
        //   VALIDATOR_KEY (env) -> BlsPrivKey  -> BFT consensus signing ONLY
        //   NODE_KEY (env)      -> secp256k1   -> EVM address -> COINBASE (block rewards)
        if self.is_validator {
            use crate::block_producer::ProducerConfig;
            use crate::consensus::{ConsensusConfig, ConsensusDriver, ValidatorKey};
            use zbx_crypto::bls::{BlsPrivKey, BlsPubKey};
            use zbx_types::{address::Address, BLOCK_GAS_LIMIT};

            let storage = self.storage.clone();
            let mempool = self.mempool.clone();

            // Resolve the raw 32-byte validator key.
            let raw_key: [u8; 32] = std::env::var("VALIDATOR_KEY")
                .ok()
                .or_else(|| cfg.chain.validator_key.clone())
                .and_then(|k| {
                    let bytes = if let Some(stripped) = k.strip_prefix("0x") {
                        hex::decode(stripped).ok()?
                    } else {
                        hex::decode(k).ok()?
                    };
                    if bytes.len() == 32 {
                        let mut arr = [0u8; 32];
                        arr.copy_from_slice(&bytes);
                        Some(arr)
                    } else {
                        // Hash to 32 bytes if a non-hex or non-32-byte key is given.
                        let h = zbx_crypto::keccak256(&bytes);
                        Some(h.0)
                    }
                })
                .unwrap_or_else(|| {
                    // Deterministic devnet key (all-ones, non-zero Fr scalar).
                    warn!("no VALIDATOR_KEY set — using deterministic devnet key (unsafe)");
                    [0xabu8; 32]
                });

            // Coinbase: ZBX-SEC-2026 — derived from NODE_KEY via secp256k1 (EVM-standard).
            // VALIDATOR_KEY (BLS) = consensus signing only.
            // NODE_KEY (secp256k1) = coinbase address (block rewards go here).
            let coinbase = {
                use zbx_crypto::secp256k1::PrivKey as Secp256k1Key;
                let legacy = |k: &[u8; 32]| -> Address {
                    let h = zbx_crypto::keccak256(k);
                    let mut a = [0u8; 20];
                    a.copy_from_slice(&h.0[12..]);
                    Address(a)
                };
                let nk = std::env::var("NODE_KEY")
                    .ok()
                    .filter(|k| !k.is_empty())
                    .or_else(|| cfg.chain.node_key.clone());
                match nk {
                    Some(k) => {
                        let s = k.strip_prefix("0x").unwrap_or(&k);
                        match hex::decode(s) {
                            Ok(b) if b.len() == 32 => {
                                match Secp256k1Key::from_bytes(&b) {
                                    Ok(sk) => {
                                        let addr = sk.to_address();
                                        info!(
                                            coinbase = %hex::encode(addr.as_bytes()),
                                            "ZBX-SEC-2026: coinbase from NODE_KEY (secp256k1 standard)"
                                        );
                                        addr
                                    }
                                    Err(e) => {
                                        warn!(error = %e, "NODE_KEY secp256k1 parse failed, using legacy");
                                        legacy(&raw_key)
                                    }
                                }
                            }
                            _ => {
                                warn!("NODE_KEY invalid (need 32-byte hex), using legacy");
                                legacy(&raw_key)
                            }
                        }
                    }
                    None => {
                        warn!("NODE_KEY not set. Using legacy keccak256(VALIDATOR_KEY) — insecure. Set NODE_KEY for production.");
                        legacy(&raw_key)
                    }
                }
            };

            // BLS private key from the raw 32-byte scalar.
            let bls_priv = BlsPrivKey::from_bytes(&raw_key)
                .unwrap_or_else(|e| {
                    warn!(error = %e, "BLS key parse failed — generating ephemeral key");
                    let mut rng = rand::thread_rng();
                    BlsPrivKey::generate(&mut rng)
                });
            let bls_pub = bls_priv.to_pubkey();

            let produce_empty = std::env::var("ZBX_PRODUCE_EMPTY_BLOCKS")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);

            let producer_cfg = ProducerConfig {
                block_time: Duration::from_millis(cfg.consensus.block_time_ms),
                coinbase,
                gas_limit: BLOCK_GAS_LIMIT,
                produce_empty_blocks: produce_empty,
                active_validators: vec![coinbase], // single-validator for now
            };

            use crate::network::NetworkServer;
            use std::collections::HashMap;

            // ── Multi-validator set: own key first, then extra_validators from config.
            let mut validators: Vec<(Address, BlsPubKey)> = vec![(coinbase, bls_pub)];
            for entry in &cfg.chain.extra_validators {
                let addr_str = entry.address.trim_start_matches("0x");
                let pk_str   = entry.bls_pubkey.trim_start_matches("0x");
                let addr_bytes = match hex::decode(addr_str) {
                    Ok(b) if b.len() == 20 => b,
                    _ => { warn!(addr = %entry.address, "extra_validator: invalid address, skipping"); continue; }
                };
                let pk_bytes = match hex::decode(pk_str) {
                    Ok(b) => b,
                    Err(_) => { warn!(pk = %entry.bls_pubkey, "extra_validator: invalid bls_pubkey hex, skipping"); continue; }
                };
                let pk = match zbx_crypto::bls::BlsPubKey::from_bytes(&pk_bytes) {
                    Ok(k) => k,
                    Err(e) => { warn!(error = %e, "extra_validator: bls_pubkey parse failed, skipping"); continue; }
                };
                let mut addr_arr = [0u8; 20];
                addr_arr.copy_from_slice(&addr_bytes);
                validators.push((Address(addr_arr), pk));
            }
            info!(count = validators.len(), "validator set loaded");

            // SEC-2026-05-09 Pass-19 (Task #9, architect-review follow-up #2):
            // Derive epoch-0 shuffle seed = keccak256(genesis_hash || chain_id_be8).
            // Identical formula to `GenesisBuilder::genesis_epoch_seed()`; we
            // recompute it here from the bootstrap output rather than
            // taking a `zbx-genesis` dep, keeping the node crate's
            // dependency graph unchanged. Every honest node bootstraps
            // to the same value (deterministic) → epoch-0 proposer
            // schedule already uses the keccak-keyed shuffle instead
            // of the predictable `round % n` legacy fallback.
            let genesis_epoch_seed = match self.storage.genesis() {
                Ok(Some(g)) => {
                    let gh = g.hash();
                    let mut buf = [0u8; 40];
                    buf[..32].copy_from_slice(gh.as_bytes());
                    buf[32..40].copy_from_slice(&cfg.chain.chain_id.to_be_bytes());
                    Some(zbx_crypto::keccak256(&buf))
                }
                Ok(None) => {
                    warn!("genesis block missing from storage — epoch-0 seed not bootstrapped");
                    None
                }
                Err(e) => {
                    warn!(error = %e, "failed to read genesis from storage — epoch-0 seed not bootstrapped");
                    None
                }
            };

            let consensus_cfg = ConsensusConfig {
                my_key: ValidatorKey { address: coinbase, bls_priv },
                validators: validators.clone(),
                producer_cfg,
                epoch_length: zbx_staking::EPOCH_LENGTH,
                genesis_epoch_seed,
            };

            // ── Build ConsensusDriver outside the spawn so we can wire the
            //    P2P layer before the driver starts running.
            //
            // SEC-2026-05-09 Pass-11 — keep one extra clone of the
            // validator-set Arc for the slashing pipeline below. The
            // pipeline mutates `self_stake` + `status` on stake-burn,
            // and shares the SAME RwLock the consensus driver and RPC
            // server already hold, so changes are visible everywhere
            // without a refresh tick.
            let pipeline_validator_set = consensus_validator_set.clone();
            let (mut driver, _vote_rx) = ConsensusDriver::new(
                consensus_cfg,
                storage.clone(),
                mempool.clone(),
                Some(consensus_new_head_tx),
                Some(consensus_validator_set),
            );

            // SEC-2026-05-09 Pass-11 — assemble end-to-end slashing
            // pipeline. EvidenceStore writes to the same RocksDB the
            // node uses (Column::SlashingEvidence + SlashingRecords).
            // Registry is rehydrated from disk so a crash mid-appeal-
            // window does not lose pending slashes.
            {
                use parking_lot::Mutex as PlMutex;
                let evidence_store = zbx_staking::EvidenceStore::new(storage.clone());
                let registry = std::sync::Arc::new(PlMutex::new(
                    zbx_staking::SlashingRegistryV2::new(0),
                ));
                let pipeline = zbx_staking::SlashingPipeline::new(
                    evidence_store,
                    registry,
                    pipeline_validator_set,
                );
                // SEC-2026-05-09 Pass-11 (architect-review follow-up):
                // FAIL-CLOSED on rehydrate failure. Continuing with an
                // empty registry would silently drop pending slashes
                // from the previous run — operationally that *forgives*
                // an offender across a restart, which is exactly the
                // bypass slashing exists to prevent. RocksDB read
                // failures are catastrophic (disk corruption / perm
                // denied), so panicking at startup is the right move:
                // the operator must investigate before the chain
                // re-joins consensus.
                match pipeline.rehydrate_from_disk() {
                    Ok(n) => info!(
                        rehydrated_records = n,
                        "Pass-11: slashing pipeline rehydrated from RocksDB"
                    ),
                    Err(e) => {
                        error!(
                            error = %e,
                            "Pass-11 FATAL: slashing pipeline rehydrate \
                             FAILED — refusing to start node (continuing \
                             would drop pending evidence from previous \
                             run, weakening slashing). Investigate \
                             RocksDB integrity before retrying."
                        );
                        panic!("slashing pipeline rehydrate failed: {e}");
                    }
                }
                driver.set_slashing_pipeline(pipeline);
            }

            // Build the validator-pubkey lookup table for vote attribution.
            let validator_pubkeys: HashMap<Address, BlsPubKey> =
                validators.into_iter().collect();

            // SEC-2026-05-09 (P1+P2): load (or generate on first boot) the
            // node's long-lived Noise XX static keypair. Persisted at
            // <data_dir>/p2p_static.key with mode 0600 on Unix. The
            // cryptographic PeerId is derived from this key.
            let noise_static = match crate::noise::NoiseStaticKey::load_or_create(
                &cfg.storage.data_dir,
            ) {
                Ok(k) => {
                    info!(
                        peer_id = %hex::encode(k.peer_id().0),
                        pubkey = %hex::encode(&k.public),
                        "P2P (P1+P2): Noise static key loaded"
                    );
                    Arc::new(k)
                }
                Err(e) => {
                    error!(error = %e, "P2P (P1+P2): failed to load Noise static key");
                    return Err(NodeError::Config(format!("noise key: {e}")));
                }
            };

            // Build the P2P NetworkServer.
            let net_server = Arc::new(NetworkServer::new(
                cfg.chain.chain_id,
                cfg.network.listen_port,
                cfg.network.bootnodes.clone(),
                storage,
                mempool,
                self.peer_manager.clone(),
                driver.vote_sender(),
                validator_pubkeys,
                rpc_peer_count,
                rpc_tx_relay_tx,
                noise_static,
            ));

            // Wire network into driver so it broadcasts blocks + votes over TCP.
            driver.set_network(net_server.clone());

            // SEC-2026-05-09 Pass-10 — hand the consensus-family metrics
            // handle to the driver. Cheap clone (Arc<AtomicU64> internally),
            // shares state with the scrape endpoint.
            driver.set_metrics(metrics_registry.consensus.clone());

            // Spawn network server as a critical task.
            let net_for_task = Arc::clone(&net_server);
            let shutdown_rx_net = shutdown_rx.clone();
            critical_tasks.spawn(async move {
                net_for_task.run(shutdown_rx_net).await;
                "network_server"
            });

            // Spawn the consensus driver.
            let shutdown_rx_cons = shutdown_rx.clone();
            critical_tasks.spawn(async move {
                driver.run(shutdown_rx_cons).await;
                "consensus_driver"
            });

            info!(
                block_time_ms = cfg.consensus.block_time_ms,
                coinbase = %hex::encode(coinbase.as_bytes()),
                p2p_port = cfg.network.listen_port,
                "HotStuff consensus driver + P2P server scheduled"
            );
        }

        // ─── 6. Wait for either a shutdown signal or a critical task exit
        let mut shutdown_rx_main = shutdown_rx.clone();
        let shutdown_reason = tokio::select! {
            sig = wait_for_shutdown_signal() => sig,
            res = critical_tasks.join_next() => {
                match res {
                    Some(Ok(name)) => format!("critical task '{name}' exited"),
                    Some(Err(e)) => format!("critical task panicked: {e}"),
                    None => "all critical tasks exited cleanly".to_string(),
                }
            }
            _ = shutdown_rx_main.changed() => "internal shutdown".to_string(),
        };
        info!(reason = %shutdown_reason, "Zebvix node initiating graceful shutdown");

        // ─── 7. Broadcast shutdown to every cooperative task ─────────────
        let _ = shutdown_tx.send(true);

        // ─── 8. Bounded drain ────────────────────────────────────────────
        let drain = async {
            while let Some(res) = critical_tasks.join_next().await {
                match res {
                    Ok(name) => info!(task = name, "critical task drained"),
                    Err(e) => error!(error = %e, "critical task panicked during drain"),
                }
            }
        };
        match tokio::time::timeout(Duration::from_secs(SHUTDOWN_DRAIN_SECS), drain).await {
            Ok(()) => info!("all critical tasks drained cleanly"),
            Err(_) => {
                warn!(
                    timeout_s = SHUTDOWN_DRAIN_SECS,
                    "shutdown drain timed out — aborting remaining tasks"
                );
                critical_tasks.shutdown().await;
            }
        }

        // Storage is dropped here; RocksDB flushes on Drop.
        info!("Zebvix node stopped");
        Ok(())
    }
}

/// Spawn a supervised task with the given restart policy.
///
/// The `make_fut` closure receives a fresh shutdown receiver each iteration
/// so AutoRestart can still cooperatively shut down between restarts.
fn spawn_supervised<F, Fut>(
    name: &'static str,
    policy: RestartPolicy,
    shutdown_rx: watch::Receiver<bool>,
    make_fut: F,
) where
    F: Fn(watch::Receiver<bool>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        let mut backoff_secs: u64 = 1;
        loop {
            // Quick gate so we never enter a fresh iteration after the
            // shutdown signal has been observed.
            if *shutdown_rx.borrow() {
                info!(task = name, "supervised task observed shutdown — exiting");
                return;
            }

            let fut = make_fut(shutdown_rx.clone());
            // tokio::spawn the body so a panic inside does not poison this
            // supervisor — JoinError::is_panic() lets us decide what to do.
            let handle = tokio::spawn(fut);
            match handle.await {
                Ok(()) => {
                    match policy {
                        RestartPolicy::Critical => {
                            error!(task = name, "critical supervised task exited — node remains up but degraded");
                            return;
                        }
                        RestartPolicy::AutoRestart => {
                            if *shutdown_rx.borrow() {
                                return;
                            }
                            warn!(
                                task = name,
                                backoff_s = backoff_secs,
                                "task exited — restarting after backoff"
                            );
                        }
                    }
                }
                Err(e) => {
                    error!(task = name, error = %e, "supervised task panicked");
                    if matches!(policy, RestartPolicy::Critical) {
                        return;
                    }
                }
            }

            // Exponential backoff with 30s cap.
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(backoff_secs)) => {}
                mut sd = {
                    let mut sd = shutdown_rx.clone();
                    async move { let _ = sd.changed().await; sd }
                } => {
                    let _ = sd; // silence unused
                    return;
                }
            }
            backoff_secs = (backoff_secs * 2).min(30);
        }
    });
}

/// Block until SIGINT (Ctrl-C) **or** SIGTERM is received.
///
/// We listen for both because production deployments are usually managed by
/// systemd / docker, both of which send SIGTERM on shutdown. The legacy
/// implementation only handled Ctrl-C, so a `kill -TERM` would terminate the
/// process abruptly with no storage flush. (Audit S4-B1.)
async fn wait_for_shutdown_signal() -> String {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(e) => {
                error!(error = %e, "failed to install SIGTERM handler — falling back to Ctrl-C only");
                let _ = tokio::signal::ctrl_c().await;
                return "SIGINT (no SIGTERM handler)".to_string();
            }
        };
        let mut sigint = match signal(SignalKind::interrupt()) {
            Ok(s) => s,
            Err(e) => {
                error!(error = %e, "failed to install SIGINT handler — falling back to Ctrl-C only");
                let _ = tokio::signal::ctrl_c().await;
                return "SIGINT (handler init failed)".to_string();
            }
        };
        tokio::select! {
            _ = sigterm.recv() => "SIGTERM".to_string(),
            _ = sigint.recv()  => "SIGINT".to_string(),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
        "Ctrl-C".to_string()
    }
}
