//! Prometheus counters.

use prometheus::{Counter, IntCounter, opts};

lazy_static::lazy_static! {
    pub static ref BLOCKS_TOTAL: IntCounter = IntCounter::new(
        "zbx_blocks_total", "Total blocks produced by this node"
    ).unwrap();

    pub static ref TXS_TOTAL: IntCounter = IntCounter::new(
        "zbx_txs_total", "Total transactions processed"
    ).unwrap();

    pub static ref CONSENSUS_TIMEOUTS: IntCounter = IntCounter::new(
        "zbx_consensus_timeouts_total", "Total consensus round timeouts"
    ).unwrap();

    pub static ref RPC_REQUESTS: IntCounter = IntCounter::new(
        "zbx_rpc_requests_total", "Total JSON-RPC requests received"
    ).unwrap();
}