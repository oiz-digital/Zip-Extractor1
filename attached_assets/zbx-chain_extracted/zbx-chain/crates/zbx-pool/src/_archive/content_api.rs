//! Mempool content API -- inspect pending/queued transactions.
//!
//! Exposes mempool state for JSON-RPC and internal block builders.
//!
//! JSON-RPC methods (zbx_* namespace):
//!   zbx_txpoolContent()         -- all pending + queued txs by sender
//!   zbx_txpoolStatus()          -- counts: { pending, queued }
//!   zbx_txpoolInspect()         -- human-readable summary
//!   zbx_getFilteredPending()    -- filter by sender / gas price / nonce range
//!
//! Also exposed on the admin HTTP endpoint (internal only):
//!   GET /admin/mempool/content
//!   GET /admin/mempool/stats
//!   POST /admin/mempool/flush   -- emergency flush (admin only)

use std::collections::HashMap;

// ── Mempool content response ──────────────────────────────────────────────────

/// Full mempool content (equivalent to eth_txpoolContent).
#[derive(Debug, Clone)]
pub struct TxPoolContent {
    /// Pending: sender -> nonce -> tx summary
    pub pending: HashMap<String, HashMap<u64, TxSummary>>,
    /// Queued: sender -> nonce -> tx summary
    pub queued:  HashMap<String, HashMap<u64, TxSummary>>,
}

/// Human-readable transaction summary for API responses.
#[derive(Debug, Clone)]
pub struct TxSummary {
    pub hash:             String,         // "0x..."
    pub from:             String,         // "0x..."
    pub to:               Option<String>, // None for contract creation
    pub value:            String,         // wei as decimal string
    pub gas:              u64,
    pub max_fee_per_gas:  String,         // wei as decimal string
    pub max_priority_fee: String,         // wei as decimal string
    pub nonce:            u64,
    pub data_size:        usize,          // bytes (not the full data)
    pub received_at:      u64,            // Unix timestamp
}

/// Mempool status (pending/queued counts).
#[derive(Debug, Clone)]
pub struct TxPoolStatus {
    pub pending: u64,
    pub queued:  u64,
}

/// Mempool metrics for monitoring (Prometheus-style).
#[derive(Debug, Clone, Default)]
pub struct MempoolStats {
    pub pending_count:          u64,
    pub queued_count:           u64,
    pub pending_bytes:          u64,   // total size in bytes
    pub queued_bytes:           u64,
    pub txs_added_total:        u64,   // counter since startup
    pub txs_removed_mined:      u64,
    pub txs_removed_evicted:    u64,
    pub txs_rejected_total:     u64,
    pub txs_replaced_by_fee:    u64,
    pub avg_pending_gas_price:  u128,
    pub min_pending_gas_price:  u128,
    pub max_pending_gas_price:  u128,
    pub unique_senders:         u64,
}

// ── Mempool content API ───────────────────────────────────────────────────────

/// Build TxPoolContent from pool state (for zbx_txpoolContent RPC).
pub fn build_pool_content(
    pending: &[(String, u64, TxSummary)],
    queued:  &[(String, u64, TxSummary)],
) -> TxPoolContent {
    let mut pending_map: HashMap<String, HashMap<u64, TxSummary>> = HashMap::new();
    for (sender, nonce, summary) in pending {
        pending_map.entry(sender.clone()).or_default().insert(*nonce, summary.clone());
    }
    let mut queued_map: HashMap<String, HashMap<u64, TxSummary>> = HashMap::new();
    for (sender, nonce, summary) in queued {
        queued_map.entry(sender.clone()).or_default().insert(*nonce, summary.clone());
    }
    TxPoolContent { pending: pending_map, queued: queued_map }
}

/// mempool_content query with optional filters.
/// Used by block builders and MEV searchers via internal API.
#[derive(Debug, Default)]
pub struct PendingInspect {
    /// Filter by sender address (hex)
    pub sender:    Option<String>,
    /// Only return txs with gas_price >= this
    pub min_gas:   Option<u128>,
    /// Only return txs with gas_price <= this
    pub max_gas:   Option<u128>,
    /// Nonce range filter
    pub nonce_min: Option<u64>,
    pub nonce_max: Option<u64>,
    /// Limit results
    pub limit:     Option<usize>,
}

/// zbx_txpoolStatus response.
pub fn pool_status(pending: &TxPoolContent) -> TxPoolStatus {
    let pending_count = pending.pending.values().map(|m| m.len() as u64).sum();
    let queued_count  = pending.queued.values().map(|m| m.len() as u64).sum();
    TxPoolStatus { pending: pending_count, queued: queued_count }
}

/// zbx_txpoolInspect -- human-readable one-liner per tx.
/// Format: "to: value wei + gas*gasPrice wei"
pub fn pool_inspect(content: &TxPoolContent) -> HashMap<String, HashMap<u64, String>> {
    let mut result = HashMap::new();
    for (sender, nonces) in &content.pending {
        let sender_map: HashMap<u64, String> = nonces.iter().map(|(nonce, tx)| {
            let to = tx.to.clone().unwrap_or_else(|| "contract_creation".into());
            let summary = format!("{}: {} wei + {} gas * {} wei/gas",
                to, tx.value, tx.gas, tx.max_fee_per_gas);
            (*nonce, summary)
        }).collect();
        result.insert(sender.clone(), sender_map);
    }
    result
}

// ── Admin flush ───────────────────────────────────────────────────────────────

/// Emergency mempool flush (admin only -- only used in extreme cases).
/// Clears all pending and queued transactions.
///
/// WARNING: This drops all unconfirmed user transactions.
/// Only used during: network attack, DoS, critical bug mitigation.
pub struct MempoolFlushCmd {
    pub admin:      [u8; 20],
    pub reason:     String,
    pub block:      u64,
    pub txs_flushed: u64,
}