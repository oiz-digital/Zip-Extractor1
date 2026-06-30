//! Gossip protocol for ZBX Chain P2P.
//!
//! ## Design
//!
//! Each node maintains a **seen-message cache** (bounded LRU).  Incoming
//! gossip messages are deduplicated before relay.  Valid unseen messages
//! are forwarded to a random subset of peers (**fan-out**).
//!
//! ```text
//! Peer A ──► [Our Node] ──► Peer B  (fan-out = 3)
//!                       ──► Peer C
//!                       ──► Peer D
//! ```
//!
//! ## Topics
//!
//! Each gossip message is tagged with a `GossipTopic` so nodes can
//! subscribe to only the topics they care about.
//!
//! | Topic           | Payload                              | Fan-out |
//! |-----------------|--------------------------------------|---------|
//! | `NewBlock`      | serialised `Block`                   | 6       |
//! | `Transaction`   | serialised `SignedTransaction`       | 4       |
//! | `ConsensusVote` | serialised `Vote`                    | all     |
//! | `TimeoutShare`  | serialised `TimeoutShare`            | all     |
//! | `Proposal`      | serialised `Proposal`                | 6       |
//!
//! ## Deduplication
//!
//! Message identity = `keccak256(topic_byte || payload)`.  The seen-cache
//! stores the last `MAX_SEEN_MESSAGES` IDs in insertion order (LRU eviction).

use crate::peer::PeerId;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use zbx_crypto::keccak::keccak256;
use zbx_types::H256;
use tracing::{debug, trace};

/// Maximum gossip messages retained in the seen-cache.
pub const MAX_SEEN_MESSAGES: usize = 4096;

/// Maximum hops a gossip message may travel (TTL field).
pub const MAX_HOPS: u8 = 7;

/// Default fan-out for non-consensus topics.
pub const DEFAULT_FANOUT: usize = 4;

/// Fan-out for consensus topics (votes / timeout shares — must reach all).
pub const CONSENSUS_FANOUT: usize = usize::MAX; // relay to ALL peers

/// Topic tag for gossip messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum GossipTopic {
    NewBlock      = 0x01,
    Transaction   = 0x02,
    ConsensusVote = 0x03,
    TimeoutShare  = 0x04,
    Proposal      = 0x05,
    TimeoutCert   = 0x06,
}

impl GossipTopic {
    pub fn fan_out(self) -> usize {
        match self {
            GossipTopic::ConsensusVote => CONSENSUS_FANOUT,
            GossipTopic::TimeoutShare  => CONSENSUS_FANOUT,
            GossipTopic::TimeoutCert   => CONSENSUS_FANOUT,
            GossipTopic::NewBlock      => 6,
            GossipTopic::Proposal      => 6,
            GossipTopic::Transaction   => DEFAULT_FANOUT,
        }
    }
}

/// A gossip message envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GossipMessage {
    /// Message topic.
    pub topic:      GossipTopic,
    /// Serialised payload (e.g. JSON-encoded block or vote).
    pub payload:    Vec<u8>,
    /// Content-hash used for deduplication.
    pub message_id: H256,
    /// Remaining hops before the message is dropped.
    pub ttl:        u8,
    /// Originating peer (for loop prevention).
    pub origin:     Option<PeerId>,
}

impl GossipMessage {
    /// Construct a new gossip message, computing the message ID.
    pub fn new(topic: GossipTopic, payload: Vec<u8>) -> Self {
        let id = Self::compute_id(topic, &payload);
        GossipMessage {
            topic,
            payload,
            message_id: id,
            ttl: MAX_HOPS,
            origin: None,
        }
    }

    /// Set the originating peer (used on inbound messages).
    pub fn with_origin(mut self, origin: PeerId) -> Self {
        self.origin = Some(origin);
        self
    }

    /// Decrement TTL. Returns `false` if the message should be dropped.
    pub fn decrement_ttl(&mut self) -> bool {
        if self.ttl == 0 {
            return false;
        }
        self.ttl -= 1;
        true
    }

    /// SEC-2026-05-09 (P6): made `pub(crate)` so the router can recompute
    /// and override sender-supplied `message_id` fields.
    pub(crate) fn compute_id(topic: GossipTopic, payload: &[u8]) -> H256 {
        let mut data = Vec::with_capacity(1 + payload.len());
        data.push(topic as u8);
        data.extend_from_slice(payload);
        keccak256(&data)
    }
}

/// Subscription set for gossip topics.
#[derive(Debug, Clone, Default)]
pub struct Subscriptions {
    topics: HashSet<GossipTopic>,
}

impl Subscriptions {
    pub fn subscribe(&mut self, topic: GossipTopic) {
        self.topics.insert(topic);
    }

    pub fn unsubscribe(&mut self, topic: GossipTopic) {
        self.topics.remove(&topic);
    }

    pub fn is_subscribed(&self, topic: GossipTopic) -> bool {
        self.topics.contains(&topic)
    }

    /// Subscribe to all consensus-critical topics.
    pub fn subscribe_consensus(&mut self) {
        self.subscribe(GossipTopic::ConsensusVote);
        self.subscribe(GossipTopic::TimeoutShare);
        self.subscribe(GossipTopic::TimeoutCert);
        self.subscribe(GossipTopic::Proposal);
    }

    /// Subscribe to all topics (full node).
    pub fn subscribe_all(&mut self) {
        self.subscribe(GossipTopic::NewBlock);
        self.subscribe(GossipTopic::Transaction);
        self.subscribe_consensus();
    }
}

/// Decision from `GossipRouter::process_inbound`.
#[derive(Debug, PartialEq, Eq)]
pub enum GossipDecision {
    /// Message is new and valid — relay to selected peers.
    Relay(Vec<PeerId>),
    /// Message was already seen — drop silently.
    Duplicate,
    /// TTL expired — drop.
    TtlExpired,
    /// Not subscribed to this topic — drop.
    NotSubscribed,
}

/// Gossip routing engine.
///
/// Manages the seen-message cache, subscriptions, and peer fan-out selection.
pub struct GossipRouter {
    /// Seen-message IDs (LRU-bounded dedup cache).
    seen:          VecDeque<H256>,
    seen_set:      HashSet<H256>,
    /// Topic subscriptions for this node.
    subscriptions: Subscriptions,
    /// Per-peer topic subscription map (for targeted relay).
    peer_topics:   HashMap<PeerId, HashSet<GossipTopic>>,
    /// Metrics counters.
    pub seen_count:      u64,
    pub relayed_count:   u64,
    pub duplicate_count: u64,
    pub dropped_count:   u64,
}

impl GossipRouter {
    pub fn new(subscriptions: Subscriptions) -> Self {
        GossipRouter {
            seen:          VecDeque::with_capacity(MAX_SEEN_MESSAGES),
            seen_set:      HashSet::new(),
            subscriptions,
            peer_topics:   HashMap::new(),
            seen_count:      0,
            relayed_count:   0,
            duplicate_count: 0,
            dropped_count:   0,
        }
    }

    /// Register (or update) a peer's topic subscriptions.
    pub fn register_peer(&mut self, peer: PeerId, topics: HashSet<GossipTopic>) {
        self.peer_topics.insert(peer, topics);
    }

    /// Remove a peer on disconnect.
    pub fn remove_peer(&mut self, peer: &PeerId) {
        self.peer_topics.remove(peer);
    }

    /// Process an inbound gossip message.  Returns the relay decision.
    pub fn process_inbound(
        &mut self,
        mut msg:      GossipMessage,
        all_peer_ids: &[PeerId],
    ) -> GossipDecision {
        // Check subscription first
        if !self.subscriptions.is_subscribed(msg.topic) {
            self.dropped_count += 1;
            trace!(topic = ?msg.topic, "gossip: not subscribed — dropping");
            return GossipDecision::NotSubscribed;
        }

        // SEC-2026-05-09 (P6): recompute the message_id from the actual
        // (topic, payload). The previous code trusted the sender-supplied
        // `message_id` field for dedup, so a hostile peer could either
        // (a) make us drop a legitimate message by replaying its id with a
        // different payload, or (b) bypass dedup entirely by sending a fresh
        // random id for every replay. We now derive the id ourselves.
        let expected_id = GossipMessage::compute_id(msg.topic, &msg.payload);
        if msg.message_id != expected_id {
            debug!(
                claimed = ?msg.message_id,
                computed = ?expected_id,
                "gossip (P6): message_id forged — using computed id"
            );
            msg.message_id = expected_id;
        }

        // SEC-2026-05-09 (P6): cap inbound TTL at MAX_HOPS so a peer can't
        // inflate it (e.g. ttl=255) and amplify gossip across the network.
        if msg.ttl > MAX_HOPS {
            msg.ttl = MAX_HOPS;
        }

        // Deduplication check
        if self.seen_set.contains(&msg.message_id) {
            self.duplicate_count += 1;
            trace!(id = ?msg.message_id, "gossip: duplicate — dropping");
            return GossipDecision::Duplicate;
        }

        // TTL check
        if !msg.decrement_ttl() {
            self.dropped_count += 1;
            debug!(id = ?msg.message_id, "gossip: TTL expired — dropping");
            return GossipDecision::TtlExpired;
        }

        // Mark as seen
        self.mark_seen(msg.message_id);
        self.seen_count += 1;

        // Select relay targets
        let targets = self.select_fanout(&msg, all_peer_ids);
        self.relayed_count += targets.len() as u64;

        debug!(
            topic     = ?msg.topic,
            id        = ?msg.message_id,
            relay_to  = targets.len(),
            "gossip: relaying"
        );

        GossipDecision::Relay(targets)
    }

    /// Create a new outbound gossip message and mark it as seen
    /// (so we don't re-process it if we receive it back).
    pub fn create_outbound(&mut self, topic: GossipTopic, payload: Vec<u8>) -> GossipMessage {
        let msg = GossipMessage::new(topic, payload);
        self.mark_seen(msg.message_id);
        msg
    }

    /// Select peers to relay a message to.
    fn select_fanout(&self, msg: &GossipMessage, all_peers: &[PeerId]) -> Vec<PeerId> {
        let fan_out = msg.topic.fan_out();
        let origin  = msg.origin.as_ref();

        // Eligible peers: subscribed to this topic, not the origin
        let mut eligible: Vec<PeerId> = all_peers.iter()
            .filter(|p| {
                let not_origin = origin.map(|o| *p != o).unwrap_or(true);
                let subscribed = self.peer_topics
                    .get(p)
                    .map(|t| t.contains(&msg.topic))
                    .unwrap_or(true); // if peer has no topic map, assume subscribed
                not_origin && subscribed
            })
            .cloned()
            .collect();

        if fan_out == CONSENSUS_FANOUT || eligible.len() <= fan_out {
            return eligible;
        }

        // Deterministic pseudo-random selection using message_id as seed
        // (avoids crypto RNG dependency in the hot path)
        let seed = u64::from_be_bytes(msg.message_id.0[..8].try_into().unwrap_or([0u8; 8]));
        pseudo_shuffle(&mut eligible, seed);
        eligible.truncate(fan_out);
        eligible
    }

    fn mark_seen(&mut self, id: H256) {
        if self.seen.len() >= MAX_SEEN_MESSAGES {
            if let Some(old) = self.seen.pop_front() {
                self.seen_set.remove(&old);
            }
        }
        self.seen.push_back(id);
        self.seen_set.insert(id);
    }

    pub fn known_seen_count(&self) -> usize {
        self.seen.len()
    }
}

/// Deterministic pseudo-shuffle (Fisher-Yates with xorshift64 PRNG).
fn pseudo_shuffle<T>(slice: &mut Vec<T>, seed: u64) {
    let mut state = if seed == 0 { 0xDEAD_BEEF_CAFE_BABEu64 } else { seed };
    for i in (1..slice.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let j = (state as usize) % (i + 1);
        slice.swap(i, j);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_peer(b: u8) -> PeerId {
        PeerId([b; 32])
    }

    fn full_subscriptions() -> Subscriptions {
        let mut s = Subscriptions::default();
        s.subscribe_all();
        s
    }

    #[test]
    fn new_message_is_relayed() {
        let mut router = GossipRouter::new(full_subscriptions());
        let msg = GossipMessage::new(GossipTopic::Transaction, b"hello".to_vec());
        let peers = vec![make_peer(1), make_peer(2), make_peer(3)];
        let decision = router.process_inbound(msg, &peers);
        assert!(matches!(decision, GossipDecision::Relay(_)));
        assert_eq!(router.seen_count, 1);
    }

    #[test]
    fn duplicate_is_dropped() {
        let mut router = GossipRouter::new(full_subscriptions());
        let msg = GossipMessage::new(GossipTopic::Transaction, b"hello".to_vec());
        let peers = vec![make_peer(1)];
        router.process_inbound(msg.clone(), &peers);
        let decision = router.process_inbound(msg, &peers);
        assert_eq!(decision, GossipDecision::Duplicate);
        assert_eq!(router.duplicate_count, 1);
    }

    #[test]
    fn ttl_zero_is_dropped() {
        let mut router = GossipRouter::new(full_subscriptions());
        let mut msg = GossipMessage::new(GossipTopic::Transaction, b"hi".to_vec());
        msg.ttl = 0;
        let decision = router.process_inbound(msg, &[]);
        assert_eq!(decision, GossipDecision::TtlExpired);
    }

    #[test]
    fn not_subscribed_drops() {
        let mut subs = Subscriptions::default();
        subs.subscribe(GossipTopic::NewBlock); // NOT Transaction
        let mut router = GossipRouter::new(subs);
        let msg = GossipMessage::new(GossipTopic::Transaction, b"tx".to_vec());
        let decision = router.process_inbound(msg, &[]);
        assert_eq!(decision, GossipDecision::NotSubscribed);
    }

    #[test]
    fn consensus_fanout_reaches_all_peers() {
        let mut router = GossipRouter::new(full_subscriptions());
        let msg = GossipMessage::new(GossipTopic::ConsensusVote, b"vote".to_vec());
        let peers: Vec<PeerId> = (1..=20u8).map(make_peer).collect();
        let decision = router.process_inbound(msg, &peers);
        if let GossipDecision::Relay(targets) = decision {
            assert_eq!(targets.len(), peers.len(), "consensus should relay to all");
        } else {
            panic!("expected Relay");
        }
    }

    #[test]
    fn origin_excluded_from_relay() {
        let mut router = GossipRouter::new(full_subscriptions());
        let origin = make_peer(99);
        let mut msg = GossipMessage::new(GossipTopic::NewBlock, b"block".to_vec());
        msg.origin = Some(origin.clone());
        let peers = vec![origin.clone(), make_peer(1), make_peer(2)];
        if let GossipDecision::Relay(targets) = router.process_inbound(msg, &peers) {
            assert!(!targets.contains(&origin), "origin must not receive its own message back");
        }
    }

    #[test]
    fn seen_cache_bounded() {
        let mut router = GossipRouter::new(full_subscriptions());
        // Add MAX_SEEN_MESSAGES + 10 unique messages
        for i in 0u64..=(MAX_SEEN_MESSAGES as u64 + 10) {
            let payload = i.to_be_bytes().to_vec();
            let msg = GossipMessage::new(GossipTopic::Transaction, payload);
            router.process_inbound(msg, &[]);
        }
        assert!(router.known_seen_count() <= MAX_SEEN_MESSAGES);
    }

    #[test]
    fn create_outbound_marks_seen() {
        let mut router = GossipRouter::new(full_subscriptions());
        let msg = router.create_outbound(GossipTopic::NewBlock, b"block".to_vec());
        let decision = router.process_inbound(msg, &[]);
        assert_eq!(decision, GossipDecision::Duplicate, "our own outbound message should be deduplicated");
    }
}
