//! Shared state passed to every RPC handler.
//!
//! Wraps the persistent storage (RocksDB-backed `ZbxDb`), the in-memory
//! mempool, the live validator set, and broadcast channels for WebSocket
//! subscriptions behind `Arc` so the RPC server can safely share them
//! across many concurrent connections.
//!
//! ## Governance persistence (H-4 fix: 2026-06-27)
//!
//! `governance_proposals` is an in-memory write-through cache backed by the
//! `GovernanceProposals` RocksDB column family.  On startup, call
//! `RpcState::load_governance_from_db()` to rehydrate the map from RocksDB
//! so proposals survive node restarts.  Every write path (`zbx_proposeGovernance`)
//! persists to RocksDB FIRST (with fsync) before updating the in-memory map.

use std::collections::HashMap;
use std::sync::Arc;
use parking_lot::RwLock;
use serde_json::Value;
use tokio::sync::broadcast;
use zbx_storage::ZbxDb;
use zbx_mempool::TransactionPool;
use zbx_staking::validator::ValidatorSet;
use zbx_types::transaction::SignedTransaction;
use tracing::{info, warn};

/// Handle to all on-node services that the RPC layer needs to read or mutate.
#[derive(Clone)]
pub struct RpcState {
    /// Persistent chain storage.
    pub db: Arc<ZbxDb>,
    /// Pending transaction pool (locked behind RwLock for write paths).
    pub mempool: Arc<RwLock<TransactionPool>>,
    /// Latest known network peer count (updated by P2P layer).
    pub peer_count: Arc<RwLock<u64>>,
    /// Whether the node is currently syncing from peers.
    pub syncing: Arc<RwLock<bool>>,
    /// Configured chain id (mainnet 8989, testnet 8990, etc.).
    pub chain_id: u64,
    /// Client version string returned by web3_clientVersion.
    pub client_version: String,

    // ── Staking ──────────────────────────────────────────────────────────────

    /// Live validator set — written by the consensus layer at each epoch
    /// boundary, read by zbx_getValidatorSet / zbx_getStakingInfo.
    pub validator_set: Arc<RwLock<ValidatorSet>>,

    // ── WebSocket subscription channels ──────────────────────────────────────

    /// Broadcast sender for new block head events (eth_subscribe "newHeads").
    /// The block producer calls `new_head_tx.send(block_json)` on each block.
    pub new_head_tx: Arc<broadcast::Sender<Value>>,

    /// Broadcast sender for new pending-transaction hashes
    /// (eth_subscribe "newPendingTransactions").
    /// The mempool calls `new_pending_tx.send(hash_hex)` on each acceptance.
    pub new_pending_tx: Arc<broadcast::Sender<String>>,

    /// Broadcast sender for TX relay over P2P.
    /// `eth_sendRawTransaction` sends every accepted TX here so the P2P layer
    /// can forward it to connected peers that may not share the same mempool.
    pub tx_relay_tx: Arc<broadcast::Sender<SignedTransaction>>,

    // ── Governance ───────────────────────────────────────────────────────────

    /// In-process governance proposal store.
    ///
    /// Keyed by `proposalId` (the `0x`-prefixed SHA-256 16-byte prefix).
    /// Each value is the full JSON object returned by `zbx_proposeGovernance`.
    ///
    /// Durability note: this store is in-process only and survives the
    /// lifetime of the node process. For cross-restart persistence, wire this
    /// to a RocksDB column family (`GovernanceProposals` CF) in a future
    /// upgrade. Sufficient for testnet and early mainnet governance.
    pub governance_proposals: Arc<RwLock<HashMap<String, Value>>>,
}

impl RpcState {
    pub fn new(
        db: Arc<ZbxDb>,
        mempool: Arc<RwLock<TransactionPool>>,
        chain_id: u64,
        client_version: impl Into<String>,
    ) -> Self {
        let (new_head_tx, _)    = broadcast::channel(128);
        let (new_pending_tx, _) = broadcast::channel(1_024);
        let (tx_relay_tx, _)    = broadcast::channel(4_096);

        Self {
            db,
            mempool,
            peer_count:    Arc::new(RwLock::new(0)),
            syncing:       Arc::new(RwLock::new(false)),
            chain_id,
            client_version: client_version.into(),
            validator_set: Arc::new(RwLock::new(ValidatorSet::new())),
            new_head_tx:    Arc::new(new_head_tx),
            new_pending_tx: Arc::new(new_pending_tx),
            tx_relay_tx:    Arc::new(tx_relay_tx),
            governance_proposals: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Rehydrate the in-memory governance proposal cache from RocksDB.
    ///
    /// MUST be called once at node startup — BEFORE the RPC server accepts
    /// requests — so that `zbx_getGovernanceProposal` returns proposals that
    /// survived a previous node restart. Safe to call more than once (each
    /// call replaces the in-memory map with the current DB contents).
    ///
    /// # H-4 fix (2026-06-27)
    /// Pre-fix: proposals were ephemeral; a restart returned `not_found` for
    /// every previously submitted proposal. This method closes that gap.
    pub fn load_governance_from_db(&self) {
        match self.db.load_all_governance_proposals() {
            Ok(loaded) => {
                let count = loaded.len();
                *self.governance_proposals.write() = loaded;
                info!(count, "governance proposals rehydrated from RocksDB");
            }
            Err(e) => {
                warn!(error = %e, "failed to rehydrate governance proposals from RocksDB \
                      — in-memory store starts empty (proposals submitted this session \
                      will still be durable via the write path)");
            }
        }
    }

    /// Best-known block height (latest written to storage), or 0 if none.
    pub fn latest_height(&self) -> u64 {
        self.db.get_latest_block_number().unwrap_or(0)
    }
}
