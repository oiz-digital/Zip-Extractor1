//! Node metrics: counters, gauges, and histograms exposed as Prometheus text.

use serde::{Serialize, Deserialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Thread-safe node metrics.
pub struct NodeMetrics {
    // Chain
    pub chain_head_number:    AtomicU64,
    pub chain_finalized:      AtomicU64,
    pub chain_reorgs_total:   AtomicU64,
    pub chain_tx_total:       AtomicU64,

    // Peers
    pub peers_total:          AtomicU64,
    pub peers_inbound:        AtomicU64,
    pub peers_outbound:       AtomicU64,
    pub peers_banned:         AtomicU64,

    // Mempool
    pub mempool_pending:      AtomicU64,
    pub mempool_queued:       AtomicU64,
    pub mempool_evictions:    AtomicU64,

    // RPC
    pub rpc_requests_total:   AtomicU64,
    pub rpc_errors_total:     AtomicU64,
    pub rpc_connections:      AtomicU64,

    // Block processing
    pub block_import_total:   AtomicU64,
    pub block_import_fails:   AtomicU64,

    // Sync
    pub sync_headers_fetched: AtomicU64,
    pub sync_bodies_fetched:  AtomicU64,
}

impl NodeMetrics {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            chain_head_number:    AtomicU64::new(0),
            chain_finalized:      AtomicU64::new(0),
            chain_reorgs_total:   AtomicU64::new(0),
            chain_tx_total:       AtomicU64::new(0),
            peers_total:          AtomicU64::new(0),
            peers_inbound:        AtomicU64::new(0),
            peers_outbound:       AtomicU64::new(0),
            peers_banned:         AtomicU64::new(0),
            mempool_pending:      AtomicU64::new(0),
            mempool_queued:       AtomicU64::new(0),
            mempool_evictions:    AtomicU64::new(0),
            rpc_requests_total:   AtomicU64::new(0),
            rpc_errors_total:     AtomicU64::new(0),
            rpc_connections:      AtomicU64::new(0),
            block_import_total:   AtomicU64::new(0),
            block_import_fails:   AtomicU64::new(0),
            sync_headers_fetched: AtomicU64::new(0),
            sync_bodies_fetched:  AtomicU64::new(0),
        })
    }

    /// Render metrics in Prometheus text exposition format.
    pub fn render_prometheus(&self) -> String {
        let ns = "zbx";
        let mut out = String::new();
        macro_rules! gauge {
            ($name:ident, $help:expr) => {
                out += &format!(
                    "# HELP {}_{} {}\n# TYPE {}_{} gauge\n{}_{} {}\n",
                    ns, stringify!($name), $help,
                    ns, stringify!($name),
                    ns, stringify!($name),
                    self.$name.load(Ordering::Relaxed)
                );
            };
        }
        macro_rules! counter {
            ($name:ident, $help:expr) => {
                out += &format!(
                    "# HELP {}_{} {}\n# TYPE {}_{} counter\n{}_{} {}\n",
                    ns, stringify!($name), $help,
                    ns, stringify!($name),
                    ns, stringify!($name),
                    self.$name.load(Ordering::Relaxed)
                );
            };
        }

        gauge!(chain_head_number,    "Current chain head block number");
        gauge!(chain_finalized,      "Last finalized block number");
        counter!(chain_reorgs_total, "Total chain reorganisations");
        counter!(chain_tx_total,     "Total transactions processed");

        gauge!(peers_total,          "Total connected peers");
        gauge!(peers_inbound,        "Inbound peer connections");
        gauge!(peers_outbound,       "Outbound peer connections");
        counter!(peers_banned,       "Total peers banned");

        gauge!(mempool_pending,      "Pending transactions in mempool");
        gauge!(mempool_queued,       "Queued transactions in mempool");
        counter!(mempool_evictions,  "Total transactions evicted from mempool");

        counter!(rpc_requests_total, "Total RPC requests received");
        counter!(rpc_errors_total,   "Total RPC errors returned");
        gauge!(rpc_connections,      "Active RPC connections");

        counter!(block_import_total, "Total blocks imported");
        counter!(block_import_fails, "Total block import failures");

        counter!(sync_headers_fetched, "Total block headers fetched during sync");
        counter!(sync_bodies_fetched,  "Total block bodies fetched during sync");

        out
    }

    // ── Increment helpers ───────────────────────────────────────────────────
    pub fn inc_block_import(&self) { self.block_import_total.fetch_add(1, Ordering::Relaxed); }
    pub fn inc_block_fail(&self)   { self.block_import_fails.fetch_add(1, Ordering::Relaxed); }
    pub fn inc_rpc_request(&self)  { self.rpc_requests_total.fetch_add(1, Ordering::Relaxed); }
    pub fn inc_rpc_error(&self)    { self.rpc_errors_total.fetch_add(1, Ordering::Relaxed); }
    pub fn inc_reorg(&self)        { self.chain_reorgs_total.fetch_add(1, Ordering::Relaxed); }
    pub fn set_head(&self, n: u64) { self.chain_head_number.store(n, Ordering::Relaxed); }
    pub fn set_finalized(&self, n: u64) { self.chain_finalized.store(n, Ordering::Relaxed); }
    pub fn set_peers(&self, total: u64, inbound: u64, outbound: u64) {
        self.peers_total.store(total, Ordering::Relaxed);
        self.peers_inbound.store(inbound, Ordering::Relaxed);
        self.peers_outbound.store(outbound, Ordering::Relaxed);
    }
    pub fn set_mempool(&self, pending: u64, queued: u64) {
        self.mempool_pending.store(pending, Ordering::Relaxed);
        self.mempool_queued.store(queued, Ordering::Relaxed);
    }
}