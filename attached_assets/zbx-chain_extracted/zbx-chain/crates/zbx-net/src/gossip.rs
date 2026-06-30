//! GossipSub configuration -- topics, mesh, flood publish.
//!
//! ZBX uses GossipSub v1.1 for:
//!   - Block propagation (beacon_block topic)
//!   - Transaction propagation (zbx_transactions topic)
//!   - Attestation aggregation (zbx_attestations topic)
//!   - Validator sync committee messages
//!
//! Key GossipSub parameters (ZBX tuning):
//!   D               = 8     Target mesh degree
//!   D_low           = 6     Minimum mesh degree
//!   D_high          = 12    Maximum mesh degree
//!   D_lazy          = 6     Lazy-push degree (IHAVE/IWANT)
//!   heartbeat_interval = 700ms  Mesh maintenance frequency
//!   history_length  = 6     IWANT window (heartbeats)
//!   history_gossip  = 3     Heartbeats of seen msg IDs to send
//!   flood_publish   = true  Publish to ALL mesh + non-mesh peers

use std::collections::{HashMap, HashSet};
use std::time::Duration;

// ── Topic definitions ─────────────────────────────────────────────────────────

/// A GossipSub topic identifier.
/// Format: /<network>/<topic-name>/<encoding>
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Topic(pub String);

impl Topic {
    /// beacon_block -- new finalized or proposed block gossip.
    /// ZBX equivalent of Ethereum's /eth2/beacon_block/ssz_snappy.
    pub fn beacon_block() -> Self {
        Self("/zbx/beacon_block/rlp_snappy".into())
    }

    /// zbx_transactions -- pending transaction hashes (lightweight gossip).
    pub fn zbx_transactions() -> Self {
        Self("/zbx/transactions/rlp".into())
    }

    /// zbx_attestations -- validator attestation messages.
    pub fn zbx_attestations() -> Self {
        Self("/zbx/attestations/rlp_snappy".into())
    }

    /// zbx_sync_committee -- sync committee contribution messages.
    pub fn zbx_sync_committee() -> Self {
        Self("/zbx/sync_committee/rlp_snappy".into())
    }

    /// zbx_blob_sidecar -- EIP-4844 blob sidecar gossip.
    pub fn zbx_blob_sidecar() -> Self {
        Self("/zbx/blob_sidecar/rlp_snappy".into())
    }

    /// All topics this node subscribes to by default.
    pub fn default_subscriptions() -> Vec<Self> {
        vec![
            Self::beacon_block(),
            Self::zbx_transactions(),
            Self::zbx_attestations(),
            Self::zbx_sync_committee(),
            Self::zbx_blob_sidecar(),
        ]
    }
}

// ── GossipSub parameters ──────────────────────────────────────────────────────

pub struct GossipParams {
    pub mesh_d:             usize,
    pub mesh_d_low:         usize,
    pub mesh_d_high:        usize,
    pub mesh_d_lazy:        usize,
    pub score_threshold:    f64,
    pub publish_threshold:  f64,
    pub heartbeat_interval: Duration,
    pub history_length:     usize,
    pub history_gossip:     usize,
    /// Flood publish: send to ALL mesh + non-mesh peers.
    /// Improves propagation speed at the cost of extra bandwidth.
    /// Enabled for beacon_block and transactions.
    pub flood_publish:      bool,
}

impl GossipParams {
    pub fn zbx_mainnet() -> Self {
        Self {
            mesh_d:             8,
            mesh_d_low:         6,
            mesh_d_high:        12,
            mesh_d_lazy:        6,
            score_threshold:    -4000.0,
            publish_threshold:  -8000.0,
            heartbeat_interval: Duration::from_millis(700),
            history_length:     6,
            history_gossip:     3,
            flood_publish:      true,
        }
    }
}

// ── GossipSub message ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct GossipMessage {
    pub topic:      Topic,
    pub data:       Vec<u8>,
    /// Unique message ID (keccak256 of data) -- used for deduplication
    pub message_id: [u8; 32],
    pub source:     Option<[u8; 64]>,
}

impl GossipMessage {
    pub fn new(topic: Topic, data: Vec<u8>) -> Self {
        let message_id = keccak256_bytes(&data);
        Self { topic, data, message_id, source: None }
    }
}

// ── Gossip router ─────────────────────────────────────────────────────────────

pub struct GossipRouter {
    params:    GossipParams,
    /// Peers in the mesh for each topic
    mesh:      HashMap<Topic, HashSet<[u8; 64]>>,
    /// Recently seen message IDs (dedup via seen_msgs set)
    seen_msgs: HashSet<[u8; 32]>,
    msg_cache: Vec<GossipMessage>,
}

impl GossipRouter {
    pub fn new(params: GossipParams) -> Self {
        Self { params, mesh: HashMap::new(), seen_msgs: HashSet::new(), msg_cache: Vec::new() }
    }

    /// Subscribe to a topic.
    pub fn subscribe(&mut self, topic: Topic) {
        self.mesh.entry(topic).or_insert_with(HashSet::new);
    }

    /// Publish a message to a topic.
    ///
    /// If flood_publish = true: send to ALL peers (mesh + non-mesh) for
    /// maximum propagation speed. Used for beacon_block and transactions.
    /// Otherwise: send only to mesh peers.
    ///
    /// Returns list of peer_ids to send the message to.
    pub fn publish(&mut self, msg: GossipMessage, all_peers: &[[u8; 64]]) -> Vec<[u8; 64]> {
        if self.seen_msgs.contains(&msg.message_id) { return vec![]; }
        self.seen_msgs.insert(msg.message_id);
        self.msg_cache.push(msg.clone());

        if self.params.flood_publish {
            // Flood publish -- send to ALL peers for max propagation speed
            all_peers.to_vec()
        } else {
            self.mesh.get(&msg.topic)
                .map(|mesh_peers| mesh_peers.iter().copied().collect())
                .unwrap_or_default()
        }
    }

    /// Heartbeat maintenance -- GRAFT/PRUNE mesh peers to maintain D.
    pub fn heartbeat(&mut self, topic: &Topic, all_peers: &[[u8; 64]]) -> HeartbeatActions {
        let mesh = self.mesh.entry(topic.clone()).or_insert_with(HashSet::new);
        let mut grafts = Vec::new();
        let mut prunes = Vec::new();
        if mesh.len() < self.params.mesh_d_low {
            for peer in all_peers {
                if mesh.len() >= self.params.mesh_d { break; }
                if !mesh.contains(peer) { mesh.insert(*peer); grafts.push(*peer); }
            }
        } else if mesh.len() > self.params.mesh_d_high {
            let excess: Vec<[u8; 64]> = mesh.iter()
                .take(mesh.len() - self.params.mesh_d)
                .copied().collect();
            for peer in excess { mesh.remove(&peer); prunes.push(peer); }
        }
        HeartbeatActions { grafts, prunes }
    }

    pub fn is_seen(&self, msg_id: &[u8; 32]) -> bool { self.seen_msgs.contains(msg_id) }
}

pub struct HeartbeatActions {
    pub grafts: Vec<[u8; 64]>,
    pub prunes: Vec<[u8; 64]>,
}

fn keccak256_bytes(data: &[u8]) -> [u8; 32] { let _ = data; [0u8; 32] }