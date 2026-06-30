//! GossipSub configuration parameters.

use std::time::Duration;

/// GossipSub configuration.
#[derive(Debug, Clone)]
pub struct GossipConfig {
    /// Target number of mesh peers (D).
    pub mesh_degree: usize,
    /// Minimum mesh peers before adding more (D_low).
    pub mesh_degree_low: usize,
    /// Maximum mesh peers before pruning (D_high).
    pub mesh_degree_high: usize,
    /// Number of peers to gossip to (D_lazy * gossip_factor).
    pub gossip_factor: f64,
    /// Heartbeat interval.
    pub heartbeat_interval: Duration,
    /// How long a fanout topic lives without a subscriber.
    pub fanout_ttl: Duration,
    /// Message cache history length (in heartbeat slots).
    pub history_length: usize,
    /// Number of gossip slots to track.
    pub history_gossip: usize,
    /// Maximum message size in bytes.
    pub max_message_size: usize,
    /// Maximum IHAVE messages per heartbeat per peer.
    pub max_ihave_length: usize,
    /// Maximum IWANT requests per heartbeat.
    pub max_iwant_requests: usize,
}

impl Default for GossipConfig {
    fn default() -> Self {
        Self {
            mesh_degree: 8,
            mesh_degree_low: 6,
            mesh_degree_high: 12,
            gossip_factor: 0.25,
            heartbeat_interval: Duration::from_secs(1),
            fanout_ttl: Duration::from_secs(60),
            history_length: 5,
            history_gossip: 3,
            max_message_size: 512 * 1024, // 512 KB
            max_ihave_length: 5000,
            max_iwant_requests: 10,
        }
    }
}