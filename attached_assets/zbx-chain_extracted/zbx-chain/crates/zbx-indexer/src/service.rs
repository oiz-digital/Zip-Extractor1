//! IndexerService — high-level node runner for the ZBX block + event indexer.
//!
//! Wires `Indexer`, `QueryEngine`, the REST API server, and the TVL snapshot
//! loop into a single long-running async task suitable for `spawn_supervised`
//! wiring in `node/src/node.rs`.
//!
//! ## What runs inside
//!
//! | Task               | Trigger           | Description                                  |
//! |--------------------|-------------------|----------------------------------------------|
//! | Block poll loop    | every 5 s         | Reads new blocks from ZbxDb → Indexer        |
//! | REST API server    | always            | `/v1/transactions`, `/v1/tvl/*`, `/healthz`  |
//! | TVL snapshot loop  | every `poll_secs` | Calls on-chain `tvlBreakdown()` → SQLite      |
//!
//! ## Backend selection
//!
//! `backend = "sqlite"` (default) stores everything in a single SQLite file at
//! `db_path`. `backend = "postgres"` is reserved for future production clusters
//! — the node logs a warning and falls back to SQLite until the backend is
//! fully implemented.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tracing::{info, warn};

use zbx_storage::ZbxDb;
use zbx_types::{Address, U256};
use zbx_sdk::provider::Provider;

use crate::{
    Indexer, IndexerConfig,
    indexer::{IndexBlock, IndexTx},
    query::QueryEngine,
    server::build_router,
    tvl::{TvlClient, snapshot_loop},
};

/// Node-level indexer configuration passed from `NodeConfig::indexer`.
#[derive(Debug, Clone)]
pub struct IndexerServiceConfig {
    /// Enable the indexer subsystem.
    pub enabled: bool,
    /// Storage backend: `"sqlite"` (default) or `"postgres"` (future).
    pub backend: String,
    /// SQLite database file path (used when `backend = "sqlite"`).
    pub db_path: String,
    /// PostgreSQL connection string (reserved; not yet implemented).
    pub postgres_url: String,
    /// Port for the REST API server.
    pub api_port: u16,
    /// Enable the REST API server.
    pub api_enabled: bool,
    /// On-chain TVL oracle contract address (hex, 0x-prefixed).
    /// Empty string → TVL snapshot loop is disabled.
    pub tvl_oracle_address: String,
    /// How often (seconds) to poll the on-chain TVL oracle.
    pub tvl_poll_secs: u64,
    /// Blocks to index per batch pass.
    pub batch_size: usize,
    /// Index internal transactions (EVM call traces).
    pub index_traces: bool,
    /// Decode ERC-20 Transfer events.
    pub decode_transfers: bool,
    /// SQLite concurrent writer threads.
    pub writer_threads: usize,
}

impl Default for IndexerServiceConfig {
    fn default() -> Self {
        Self {
            enabled: false,             // opt-in — heavy I/O, operators enable explicitly
            backend: "sqlite".into(),
            db_path: "./zbx-index.db".into(),
            postgres_url: String::new(),
            api_port: 3100,
            api_enabled: true,
            tvl_oracle_address: String::new(),
            tvl_poll_secs: 60,
            batch_size: 100,
            index_traces: true,
            decode_transfers: true,
            writer_threads: 4,
        }
    }
}

/// High-level indexer service.
pub struct IndexerService {
    cfg:     IndexerServiceConfig,
    storage: Arc<ZbxDb>,
    rpc_url: String,        // local node RPC URL for TVL eth_call
}

impl IndexerService {
    /// Create a new `IndexerService`.
    ///
    /// * `cfg`     — merged from `NodeConfig::indexer`.
    /// * `storage` — shared RocksDB handle for reading newly-produced blocks.
    /// * `rpc_url` — local HTTP RPC URL (e.g. `"http://127.0.0.1:8545"`) used
    ///               by the TVL snapshot loop for on-chain `eth_call`.
    pub fn new(cfg: IndexerServiceConfig, storage: Arc<ZbxDb>, rpc_url: String) -> Self {
        Self { cfg, storage, rpc_url }
    }

    /// Run the indexer service until the shutdown signal fires.
    pub async fn run_until_shutdown(
        self,
        shutdown: &mut watch::Receiver<bool>,
    ) -> Result<(), String> {
        if self.cfg.backend == "postgres" {
            warn!(
                "indexer: PostgreSQL backend is not yet implemented — \
                 falling back to SQLite at {}",
                self.cfg.db_path
            );
        }

        // Build the underlying indexer (opens / initializes the SQLite DB).
        let indexer_cfg = IndexerConfig {
            db_path:          self.cfg.db_path.clone(),
            batch_size:       self.cfg.batch_size,
            index_traces:     self.cfg.index_traces,
            decode_transfers: self.cfg.decode_transfers,
            writer_threads:   self.cfg.writer_threads,
        };

        let indexer = Indexer::new(indexer_cfg).await
            .map_err(|e| format!("indexer init: {e}"))?;

        // Determine where to resume indexing.
        let mut last_indexed = indexer
            .highest_block()
            .await
            .unwrap_or(None)
            .unwrap_or(0);

        info!(
            db_path      = %self.cfg.db_path,
            resume_from  = last_indexed + 1,
            api_port     = self.cfg.api_port,
            tvl_oracle   = %self.cfg.tvl_oracle_address,
            "indexer service started"
        );

        // ── REST API server ────────────────────────────────────────────────
        // Clone the connection for QueryEngine; Indexer keeps the other clone.
        let engine = Arc::new(QueryEngine::new(indexer.connection().clone()));

        if self.cfg.api_enabled {
            let router   = build_router(Arc::clone(&engine));
            let api_port = self.cfg.api_port;
            let addr: SocketAddr = format!("0.0.0.0:{api_port}").parse()
                .map_err(|e| format!("indexer api addr: {e}"))?;

            tokio::spawn(async move {
                let listener = tokio::net::TcpListener::bind(addr).await
                    .expect("indexer: failed to bind API port");
                info!(port = api_port, "indexer REST API listening");
                if let Err(e) = axum::serve(listener, router).await {
                    warn!("indexer REST API exited: {}", e);
                }
            });
        }

        // ── TVL snapshot loop ──────────────────────────────────────────────
        if !self.cfg.tvl_oracle_address.is_empty() {
            let tvl_conn     = indexer.connection().clone();
            let tvl_poll     = self.cfg.tvl_poll_secs;
            let oracle_hex   = self.cfg.tvl_oracle_address.clone();
            let rpc_url      = self.rpc_url.clone();

            tokio::spawn(async move {
                // Parse oracle address from hex string.
                let addr_bytes = hex::decode(
                    oracle_hex.trim_start_matches("0x")
                ).unwrap_or_else(|_| vec![0u8; 20]);
                let mut arr = [0u8; 20];
                let len = addr_bytes.len().min(20);
                arr[20 - len..].copy_from_slice(&addr_bytes[..len]);
                let oracle = Address(arr);

                match Provider::http(&rpc_url).await {
                    Ok(provider) => {
                        let client = TvlClient::new(provider, oracle);
                        if let Err(e) = snapshot_loop(client, tvl_conn, tvl_poll).await {
                            warn!("indexer tvl snapshot loop exited: {}", e);
                        }
                    }
                    Err(e) => {
                        warn!(
                            rpc_url = %rpc_url,
                            "indexer: could not connect provider for TVL loop: {}; TVL tracking disabled",
                            e
                        );
                    }
                }
            });
        }

        // ── Block polling loop ─────────────────────────────────────────────
        let storage: Arc<zbx_storage::ZbxDb> = Arc::clone(&self.storage);
        let batch   = self.cfg.batch_size;

        let mut poll_tick = tokio::time::interval(Duration::from_secs(5));
        poll_tick.tick().await; // skip immediate first tick

        loop {
            tokio::select! {
                _ = poll_tick.tick() => {
                    let chain_tip = storage.get_latest_block_number().unwrap_or(0);

                    if chain_tip <= last_indexed {
                        tracing::trace!(head = chain_tip, "indexer: up to date");
                        continue;
                    }

                    // Index blocks [last_indexed + 1 .. min(chain_tip, last_indexed + batch)]
                    let from = last_indexed + 1;
                    let to   = chain_tip.min(last_indexed + batch as u64);

                    let mut idx_blocks: Vec<IndexBlock> = Vec::with_capacity((to - from + 1) as usize);

                    for n in from..=to {
                        match storage.get_block_by_number(n) {
                            Ok(Some(block)) => {
                                let idx_block = block_to_index_block(&block);
                                idx_blocks.push(idx_block);
                            }
                            Ok(None) => {
                                tracing::debug!(block = n, "indexer: block not yet in storage");
                                break;
                            }
                            Err(e) => {
                                warn!(block = n, error = %e, "indexer: storage read error");
                                break;
                            }
                        }
                    }

                    if !idx_blocks.is_empty() {
                        let indexed_up_to = idx_blocks.last().map(|b| b.number).unwrap_or(last_indexed);
                        match indexer.index_blocks(idx_blocks).await {
                            Ok(()) => {
                                tracing::debug!(from, to = indexed_up_to, "indexer: batch complete");
                                last_indexed = indexed_up_to;
                            }
                            Err(e) => {
                                warn!(from, error = %e, "indexer: batch write failed");
                            }
                        }
                    }
                }
                _ = shutdown.changed() => {
                    info!("indexer service received shutdown signal");
                    return Ok(());
                }
            }
        }
    }
}

// ─── Block conversion ─────────────────────────────────────────────────────────

/// Convert a `zbx_types::Block` into the `IndexBlock` format expected by the
/// indexer's SQLite writer.
///
/// Note: per-transaction `gas_used`, `success`, and event `logs` require
/// execution receipts stored separately. Until receipt storage is queryable
/// from `ZbxDb`, these fields use sensible zero-values; the tx hash, sender,
/// recipient, value, gas_limit, nonce, and input are fully populated.
fn block_to_index_block(block: &zbx_types::Block) -> IndexBlock {
    let txs: Vec<IndexTx> = block
        .body
        .transactions
        .iter()
        .enumerate()
        .map(|(idx, stx)| signed_tx_to_index_tx(stx, idx))
        .collect();

    IndexBlock {
        number:      block.header.number,
        hash:        block.hash(),
        parent_hash: block.header.parent_hash,
        timestamp:   block.header.timestamp,
        gas_used:    block.header.gas_used,
        gas_limit:   block.header.gas_limit,
        base_fee:    U256::from(block.header.base_fee_per_gas),
        coinbase:    block.header.coinbase,
        state_root:  block.header.state_root,
        txs,
    }
}

/// Convert a `zbx_types::SignedTransaction` into `IndexTx`.
///
/// All fields are read from the public `SignedTransaction` / `Transaction`
/// struct fields directly — no method calls needed.
/// `gas_used`, `success`, and `logs` require execution receipts; they are
/// seeded with safe defaults until `ZbxDb::get_receipt` is wired.
fn signed_tx_to_index_tx(stx: &zbx_types::SignedTransaction, tx_index: usize) -> IndexTx {
    IndexTx {
        hash:          stx.hash,
        tx_index,
        from:          stx.from,
        to:            stx.tx.to,
        value:         stx.tx.value,
        gas_limit:     stx.tx.gas_limit,
        gas_used:      0,           // populated from receipts (future: ZbxDb::get_receipt)
        gas_price:     U256::from(stx.tx.max_fee_per_gas),
        nonce:         stx.tx.nonce,
        input:         stx.tx.data.clone(),
        success:       true,        // optimistic; overridden when receipts are available
        contract_addr: None,        // set for CREATE txs when receipts are indexed
        logs:          Vec::new(),  // populated from receipts (future)
    }
}
