//! Prometheus metrics registry for Zebvix Chain.

use prometheus::{
    Counter, CounterVec, Gauge, GaugeVec, Histogram, HistogramOpts, HistogramVec,
    IntCounter, IntCounterVec, IntGauge, IntGaugeVec,
    Opts, Registry,
    register_counter_vec_with_registry,
    register_int_counter_with_registry,
    register_int_counter_vec_with_registry,
    register_int_gauge_with_registry,
    register_int_gauge_vec_with_registry,
    register_histogram_vec_with_registry,
};
use once_cell::sync::OnceCell;

static METRICS: OnceCell<ZbxMetrics> = OnceCell::new();

/// All Zebvix Chain Prometheus metrics.
pub struct ZbxMetrics {
    // ─── Chain ────────────────────────────────────────────────────────────
    /// Total blocks imported.
    pub blocks_imported:      IntCounter,
    /// Current chain height.
    pub chain_height:         IntGauge,
    /// Block processing time (ms).
    pub block_process_time:   HistogramVec,
    /// Transactions per block.
    pub txs_per_block:        HistogramVec,
    /// Gas used per block.
    pub gas_per_block:        HistogramVec,

    // ─── Consensus ────────────────────────────────────────────────────────
    /// Consensus rounds started.
    pub consensus_rounds:     IntCounter,
    /// Consensus view changes (timeouts).
    pub view_changes:         IntCounter,
    /// Time to reach QC (ms).
    pub qc_latency:           HistogramVec,
    /// Current consensus view.
    pub consensus_view:       IntGauge,

    // ─── Mempool ──────────────────────────────────────────────────────────
    /// Pending transactions in pool.
    pub mempool_pending:      IntGauge,
    /// Transactions rejected from pool.
    pub mempool_rejected:     IntCounterVec,
    /// Transactions added to pool.
    pub mempool_added:        IntCounter,
    /// Transactions removed (committed).
    pub mempool_committed:    IntCounter,

    // ─── Network ──────────────────────────────────────────────────────────
    /// Connected peers.
    pub peer_count:           IntGauge,
    /// Bytes sent.
    pub bytes_sent:           IntCounter,
    /// Bytes received.
    pub bytes_received:       IntCounter,
    /// Messages sent per type.
    pub messages_sent:        IntCounterVec,
    /// Messages received per type.
    pub messages_received:    IntCounterVec,

    // ─── RPC ──────────────────────────────────────────────────────────────
    /// RPC requests per method.
    pub rpc_requests:         IntCounterVec,
    /// RPC errors per method.
    pub rpc_errors:           IntCounterVec,
    /// RPC latency per method.
    pub rpc_latency:          HistogramVec,

    // ─── EVM ──────────────────────────────────────────────────────────────
    /// EVM invocations (calls + creates).
    pub evm_invocations:      IntCounter,
    /// EVM out-of-gas events.
    pub evm_out_of_gas:       IntCounter,
    /// EVM reverts.
    pub evm_reverts:          IntCounter,
    /// Gas used per EVM call.
    pub evm_gas_used:         HistogramVec,
}

impl ZbxMetrics {
    pub fn new(node_id: &str) -> Result<Self, prometheus::Error> {
        let labels_map: std::collections::HashMap<String, String> =
            [("node".to_string(), node_id.to_string())].into_iter().collect();
        let labels = &labels_map;
        let registry = prometheus::default_registry();

        macro_rules! ic {
            ($name:expr, $help:expr) => {
                register_int_counter_with_registry!(
                    Opts::new($name, $help).const_labels(labels.clone()),
                    registry
                )?
            };
        }

        macro_rules! ig {
            ($name:expr, $help:expr) => {
                register_int_gauge_with_registry!(
                    Opts::new($name, $help).const_labels(labels.clone()),
                    registry
                )?
            };
        }

        macro_rules! icv {
            ($name:expr, $help:expr, $labels:expr) => {
                register_int_counter_vec_with_registry!(
                    Opts::new($name, $help).const_labels(labels.clone()),
                    $labels,
                    registry
                )?
            };
        }

        macro_rules! hv {
            ($name:expr, $help:expr, $labels:expr, $buckets:expr) => {
                register_histogram_vec_with_registry!(
                    HistogramOpts::new($name, $help)
                        .buckets($buckets)
                        .const_labels(labels.clone()),
                    $labels,
                    registry
                )?
            };
        }

        let ms_buckets = vec![1.0, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 5000.0];
        let gas_buckets = vec![21_000.0, 100_000.0, 500_000.0, 1_000_000.0, 5_000_000.0, 15_000_000.0, 30_000_000.0];

        Ok(Self {
            blocks_imported:    ic!("zbx_blocks_imported_total",   "Total blocks imported"),
            chain_height:       ig!("zbx_chain_height",            "Current chain height"),
            block_process_time: hv!("zbx_block_process_ms",        "Block processing time (ms)", &["stage"], ms_buckets.clone()),
            txs_per_block:      hv!("zbx_txs_per_block",           "Transactions per block", &["chain"], vec![0.0,10.0,50.0,100.0,500.0,1000.0,5000.0]),
            gas_per_block:      hv!("zbx_gas_per_block",           "Gas used per block", &["chain"], gas_buckets.clone()),
            consensus_rounds:   ic!("zbx_consensus_rounds_total",  "Consensus rounds started"),
            view_changes:       ic!("zbx_view_changes_total",      "Consensus view changes"),
            qc_latency:         hv!("zbx_qc_latency_ms",           "Time to reach QC (ms)", &["chain"], ms_buckets.clone()),
            consensus_view:     ig!("zbx_consensus_view",          "Current consensus view"),
            mempool_pending:    ig!("zbx_mempool_pending",         "Pending transactions"),
            mempool_rejected:   icv!("zbx_mempool_rejected_total", "Rejected transactions", &["reason"]),
            mempool_added:      ic!("zbx_mempool_added_total",     "Transactions added to pool"),
            mempool_committed:  ic!("zbx_mempool_committed_total", "Transactions committed"),
            peer_count:         ig!("zbx_peer_count",              "Connected peers"),
            bytes_sent:         ic!("zbx_bytes_sent_total",        "Total bytes sent"),
            bytes_received:     ic!("zbx_bytes_received_total",    "Total bytes received"),
            messages_sent:      icv!("zbx_messages_sent_total",    "Messages sent", &["type"]),
            messages_received:  icv!("zbx_messages_received_total","Messages received", &["type"]),
            rpc_requests:       icv!("zbx_rpc_requests_total",     "RPC requests", &["method"]),
            rpc_errors:         icv!("zbx_rpc_errors_total",       "RPC errors", &["method"]),
            rpc_latency:        hv!("zbx_rpc_latency_ms",          "RPC latency (ms)", &["method"], ms_buckets.clone()),
            evm_invocations:    ic!("zbx_evm_invocations_total",   "EVM invocations"),
            evm_out_of_gas:     ic!("zbx_evm_out_of_gas_total",    "EVM out-of-gas events"),
            evm_reverts:        ic!("zbx_evm_reverts_total",       "EVM reverts"),
            evm_gas_used:       hv!("zbx_evm_gas_used",            "Gas used per EVM call", &["type"], gas_buckets),
        })
    }

    pub fn global() -> Option<&'static Self> { METRICS.get() }

    pub fn set_global(metrics: Self) -> Result<(), Self> {
        METRICS.set(metrics)
    }
}