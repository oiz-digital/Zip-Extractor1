//! Prometheus gauges.

use prometheus::IntGauge;

lazy_static::lazy_static! {
    pub static ref MEMPOOL_SIZE: IntGauge = IntGauge::new(
        "zbx_mempool_size", "Current number of pending transactions"
    ).unwrap();

    pub static ref PEER_COUNT: IntGauge = IntGauge::new(
        "zbx_peer_count", "Number of connected peers"
    ).unwrap();

    pub static ref CHAIN_HEIGHT: IntGauge = IntGauge::new(
        "zbx_chain_height", "Current chain head height"
    ).unwrap();

    pub static ref CONSENSUS_ROUND: IntGauge = IntGauge::new(
        "zbx_consensus_round", "Current consensus round number"
    ).unwrap();

    pub static ref VALIDATOR_ACTIVE: IntGauge = IntGauge::new(
        "zbx_validators_active", "Number of active validators"
    ).unwrap();
}