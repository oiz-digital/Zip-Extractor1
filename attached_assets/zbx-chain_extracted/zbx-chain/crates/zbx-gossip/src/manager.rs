//! GossipSub manager: mesh maintenance, heartbeat, and message dispatch.

use crate::{
    config::GossipConfig,
    topic::Topic,
    peer_score::{PeerScorer, ScoreParams},
    message_cache::{MessageCache, CachedMessage},
};
use zbx_types::H256;
use std::collections::{HashMap, HashSet};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// Events emitted by the gossip manager.
#[derive(Debug)]
pub enum GossipEvent {
    /// A new message was received on a topic.
    Received { topic: Topic, data: Vec<u8>, from: H256 },
    /// A peer was added to the mesh.
    PeerAdded(H256),
    /// A peer was pruned from the mesh.
    PeerPruned(H256),
}

/// Control messages to the gossip manager.
#[derive(Debug)]
pub enum GossipCommand {
    /// Subscribe to a topic.
    Subscribe(Topic),
    /// Unsubscribe from a topic.
    Unsubscribe(Topic),
    /// Publish a message.
    Publish { topic: Topic, data: Vec<u8> },
    /// Add a known peer.
    AddPeer(H256),
    /// Remove a peer.
    RemovePeer(H256),
    /// Heartbeat tick.
    Heartbeat,
}

/// The GossipSub manager.
pub struct GossipManager {
    config:     GossipConfig,
    /// Topics we are subscribed to and their mesh peers.
    mesh:       HashMap<Topic, HashSet<H256>>,
    /// All known peers.
    all_peers:  HashSet<H256>,
    /// Message cache for deduplication.
    msg_cache:  MessageCache,
    /// Peer scorer.
    scorer:     PeerScorer,
    event_tx:   mpsc::Sender<GossipEvent>,
}

impl GossipManager {
    pub fn new(config: GossipConfig) -> (Self, mpsc::Receiver<GossipEvent>) {
        let (event_tx, event_rx) = mpsc::channel(1024);
        let msg_cache = MessageCache::new(config.history_length, 100_000);
        let mgr = Self {
            config,
            mesh: HashMap::new(),
            all_peers: HashSet::new(),
            msg_cache,
            scorer: PeerScorer::new(ScoreParams::default()),
            event_tx,
        };
        (mgr, event_rx)
    }

    pub fn subscribe(&mut self, topic: Topic) {
        info!("gossip: subscribing to {}", topic);
        self.mesh.entry(topic).or_default();
    }

    pub fn unsubscribe(&mut self, topic: &Topic) {
        info!("gossip: unsubscribing from {}", topic);
        self.mesh.remove(topic);
    }

    pub fn add_peer(&mut self, peer: H256) {
        self.all_peers.insert(peer);
        // Try to add to mesh if below D.
        for (topic, mesh) in self.mesh.iter_mut() {
            if mesh.len() < self.config.mesh_degree {
                mesh.insert(peer);
                debug!("gossip: added {} to mesh for {}", peer, topic);
            }
        }
    }

    pub fn remove_peer(&mut self, peer: &H256) {
        self.all_peers.remove(peer);
        for mesh in self.mesh.values_mut() {
            mesh.remove(peer);
        }
    }

    pub fn publish(&mut self, topic: &Topic, data: Vec<u8>) {
        use xxhash_rust::xxh64::xxh64;
        let id_bytes = xxh64(&data, 0).to_be_bytes();
        let mut id_arr = [0u8; 32];
        id_arr[..8].copy_from_slice(&id_bytes);
        let id = H256(id_arr);

        // NODE-SEC-2026: use the current heartbeat slot so the sliding-window
        // eviction in MessageCache.advance_slot() can actually expire this entry.
        // Previously slot was hardcoded to 0, meaning messages were never evicted
        // by the window and the cache could grow without bound until the LRU cap.
        let current_slot = self.msg_cache.current_slot();
        if !self.msg_cache.insert(CachedMessage {
            id,
            topic: topic.to_string(),
            data: data.clone(),
            slot: current_slot,
        }) {
            return; // Already seen.
        }

        // Forward to mesh peers.
        if let Some(mesh) = self.mesh.get(topic) {
            debug!("gossip: publishing to {} mesh peers", mesh.len());
            // In production: send message to all mesh peers.
        }
    }

    pub fn receive(&mut self, from: H256, topic: Topic, data: Vec<u8>, id: H256) {
        if self.msg_cache.seen(&id) {
            self.scorer.on_duplicate(from, topic.as_str());
            return;
        }

        self.scorer.on_first_delivery(from, topic.as_str());
        // NODE-SEC-2026: stamp with current slot so advance_slot() evicts correctly.
        let current_slot = self.msg_cache.current_slot();
        self.msg_cache.insert(CachedMessage {
            id,
            topic: topic.to_string(),
            data: data.clone(),
            slot: current_slot,
        });

        // Emit event.
        let _ = self.event_tx.try_send(GossipEvent::Received { topic, data, from });
    }

    pub fn heartbeat(&mut self) {
        self.msg_cache.advance_slot();
        // Prune and graft mesh peers to maintain D.
        for (topic, mesh) in self.mesh.iter_mut() {
            // Prune if above D_high.
            while mesh.len() > self.config.mesh_degree_high {
                if let Some(&peer) = mesh.iter().next() {
                    if self.scorer.should_prune(peer) {
                        mesh.remove(&peer);
                        warn!("gossip: pruned low-score peer from {}", topic);
                    } else {
                        break;
                    }
                } else { break; }
            }
            // Graft if below D_low.
            if mesh.len() < self.config.mesh_degree_low {
                let candidates: Vec<_> = self.all_peers.iter()
                    .filter(|p| !mesh.contains(*p))
                    .copied()
                    .collect();
                let needed = self.config.mesh_degree.saturating_sub(mesh.len());
                for peer in candidates.into_iter().take(needed) {
                    mesh.insert(peer);
                    debug!("gossip: grafted peer into {} mesh", topic);
                }
            }
        }
    }
}