//! Consensus gossip layer: fan-out broadcast for votes, proposals, and TCs.
//!
//! ## Design
//!
//! The gossip module provides a lightweight, pluggable fan-out layer for
//! consensus messages. It is NOT responsible for transport (that lives in
//! `zbx-network` / `zbx-net`); instead it owns the *policy* decisions:
//!
//! * Which peers should receive which messages (fan-out set selection)
//! * Deduplication of inbound messages (seen-set with LRU eviction)
//! * Rate-limiting per peer per message type (guards against spam)
//! * Priority ordering: PROPOSAL > QC > TC > VOTE > TIMEOUT_SHARE
//!
//! ## Fan-out strategy
//!
//! For a validator set of n nodes we use a square-root fan-out:
//!
//! ```text
//! fan_out = max(MIN_FANOUT, ceil(sqrt(n)))
//! ```
//!
//! Each node selects `fan_out` peers uniformly at random from the current
//! validator set (excluding itself) and forwards each inbound message once.
//! With high probability this reaches all honest nodes in O(log n) hops.
//!
//! ## Deduplication
//!
//! Inbound messages are keyed by (type, round, sender_addr).  A seen-set
//! of capacity `SEEN_CAPACITY` with FIFO eviction prevents replay floods.
//! Messages older than `EXPIRY_ROUNDS` rounds behind the current tip are
//! silently dropped.

use crate::{
    error::ConsensusError,
    pacemaker::{TimeoutShare, TimeoutCertificate},
    vote::{Vote, QuorumCertificate},
};
use zbx_types::{address::Address, block::Block, H256};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use tracing::{debug, trace, warn};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Minimum fan-out peers regardless of validator-set size.
const MIN_FANOUT: usize = 4;
/// Maximum seen-set size (FIFO eviction beyond this).
const SEEN_CAPACITY: usize = 8_192;
/// Drop messages more than this many rounds behind the current tip.
const EXPIRY_ROUNDS: u64 = 10;
/// Maximum inbound messages per peer per second (anti-spam).
const RATE_LIMIT_PER_SEC: u32 = 200;

// ── Message types ─────────────────────────────────────────────────────────────

/// Priority ordering for outbound queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum GossipPriority {
    TimeoutShare = 0,
    Vote         = 1,
    Tc           = 2,
    Qc           = 3,
    Proposal     = 4,
}

/// All message types that the consensus gossip layer exchanges.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GossipMessage {
    Proposal {
        block: Box<Block>,
        round: u64,
        epoch: u64,
        proposer: Address,
    },
    Vote(Vote),
    Qc(QuorumCertificate),
    Tc(TimeoutCertificate),
    TimeoutShare(TimeoutShare),
}

impl GossipMessage {
    pub fn priority(&self) -> GossipPriority {
        match self {
            GossipMessage::Proposal { .. } => GossipPriority::Proposal,
            GossipMessage::Qc(_)           => GossipPriority::Qc,
            GossipMessage::Tc(_)           => GossipPriority::Tc,
            GossipMessage::Vote(_)         => GossipPriority::Vote,
            GossipMessage::TimeoutShare(_) => GossipPriority::TimeoutShare,
        }
    }

    pub fn round(&self) -> u64 {
        match self {
            GossipMessage::Proposal { round, .. } => *round,
            GossipMessage::Vote(v)                => v.data.block_number,
            GossipMessage::Qc(q)                  => q.block_number(),
            GossipMessage::Tc(t)                  => t.round,
            GossipMessage::TimeoutShare(ts)       => ts.data.round,
        }
    }

    /// Unique dedup key: (discriminant_byte, round, sender).
    pub fn dedup_key(&self) -> [u8; 41] {
        let mut key = [0u8; 41];
        key[0] = self.priority() as u8;
        key[1..9].copy_from_slice(&self.round().to_be_bytes());
        // Fill sender address (last 20 bytes, offset 21)
        match self {
            GossipMessage::Vote(v) => {
                key[21..41].copy_from_slice(v.voter.as_bytes());
            }
            GossipMessage::TimeoutShare(ts) => {
                key[21..41].copy_from_slice(ts.validator.as_bytes());
            }
            _ => {}
        }
        key
    }
}

// ── Outbound envelope ─────────────────────────────────────────────────────────

/// An outbound gossip message with its intended recipients.
#[derive(Debug)]
pub struct Envelope {
    pub message: GossipMessage,
    pub recipients: Vec<Address>,
}

// ── Peer rate-limiter ─────────────────────────────────────────────────────────

struct PeerLimiter {
    count: u32,
    window_start_secs: u64,
}

impl PeerLimiter {
    fn new() -> Self { PeerLimiter { count: 0, window_start_secs: 0 } }

    fn check_and_record(&mut self, now_secs: u64) -> bool {
        if now_secs != self.window_start_secs {
            self.count = 0;
            self.window_start_secs = now_secs;
        }
        if self.count >= RATE_LIMIT_PER_SEC {
            return false;
        }
        self.count += 1;
        true
    }
}

// ── GossipEngine ─────────────────────────────────────────────────────────────

/// The consensus gossip engine.
pub struct GossipEngine {
    /// This node's address.
    self_addr: Address,
    /// Active validator set (addresses, in epoch order).
    validators: Vec<Address>,
    /// Fan-out count.
    fan_out: usize,
    /// Seen-message dedup set.
    seen: VecDeque<[u8; 41]>,
    seen_set: HashSet<[u8; 41]>,
    /// Per-peer rate limiters.
    rate_limiters: HashMap<Address, PeerLimiter>,
    /// Current consensus round (used for expiry checks).
    current_round: u64,
}

impl GossipEngine {
    /// Construct a new gossip engine for a given validator set.
    pub fn new(self_addr: Address, validators: Vec<Address>) -> Self {
        let n = validators.len();
        let fan_out = (n as f64).sqrt().ceil() as usize;
        let fan_out = fan_out.max(MIN_FANOUT).min(n.saturating_sub(1));
        GossipEngine {
            self_addr,
            validators,
            fan_out,
            seen: VecDeque::with_capacity(SEEN_CAPACITY),
            seen_set: HashSet::with_capacity(SEEN_CAPACITY),
            rate_limiters: HashMap::new(),
            current_round: 0,
        }
    }

    /// Update the current round so expired messages are detected.
    pub fn set_round(&mut self, round: u64) {
        self.current_round = round;
    }

    /// Receive an inbound message from `sender`.
    ///
    /// Returns `Ok(Some(envelope))` if the message should be forwarded.
    /// Returns `Ok(None)` if the message is a duplicate or expired.
    /// Returns `Err` if it should be penalised (rate-limit exceeded).
    pub fn on_inbound(
        &mut self,
        sender: &Address,
        msg: GossipMessage,
        now_secs: u64,
    ) -> Result<Option<Envelope>, ConsensusError> {
        // Rate-limit check.
        let limiter = self
            .rate_limiters
            .entry(sender.clone())
            .or_insert_with(PeerLimiter::new);
        if !limiter.check_and_record(now_secs) {
            warn!(?sender, "gossip: rate limit exceeded — dropping message");
            return Err(ConsensusError::RateLimitExceeded);
        }

        // Expiry check.
        let msg_round = msg.round();
        if msg_round + EXPIRY_ROUNDS < self.current_round {
            trace!(msg_round, current = self.current_round, "gossip: expired message dropped");
            return Ok(None);
        }

        // Dedup check.
        let key = msg.dedup_key();
        if self.seen_set.contains(&key) {
            trace!("gossip: duplicate message dropped");
            return Ok(None);
        }
        self.insert_seen(key);

        // Select fan-out recipients.
        let recipients = self.select_peers(sender);
        if recipients.is_empty() {
            return Ok(None);
        }

        debug!(
            priority = ?msg.priority(),
            round = msg_round,
            recipients = recipients.len(),
            "gossip: forwarding message"
        );
        Ok(Some(Envelope { message: msg, recipients }))
    }

    /// Create an outbound broadcast (self-originated message).
    pub fn broadcast(&mut self, msg: GossipMessage) -> Envelope {
        let key = msg.dedup_key();
        self.insert_seen(key);
        let dummy = self.self_addr.clone();
        let recipients = self.select_peers(&dummy);
        Envelope { message: msg, recipients }
    }

    /// Update the validator set after an epoch change.
    pub fn update_validators(&mut self, validators: Vec<Address>) {
        let n = validators.len();
        self.fan_out = ((n as f64).sqrt().ceil() as usize)
            .max(MIN_FANOUT)
            .min(n.saturating_sub(1));
        self.validators = validators;
        // Clear stale rate-limiters for departed validators.
        self.rate_limiters.retain(|a, _| self.validators.contains(a));
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn insert_seen(&mut self, key: [u8; 41]) {
        if self.seen.len() >= SEEN_CAPACITY {
            if let Some(old) = self.seen.pop_front() {
                self.seen_set.remove(&old);
            }
        }
        self.seen.push_back(key);
        self.seen_set.insert(key);
    }

    /// Select `fan_out` peers at random from the validator set, excluding
    /// self and the message sender.
    fn select_peers(&self, exclude: &Address) -> Vec<Address> {
        let candidates: Vec<&Address> = self
            .validators
            .iter()
            .filter(|a| *a != &self.self_addr && *a != exclude)
            .collect();
        if candidates.is_empty() {
            return vec![];
        }
        // Deterministic shuffle using round as seed (avoids external rand dep).
        let seed = self.current_round;
        let n = candidates.len();
        let mut indices: Vec<usize> = (0..n).collect();
        // Simple Fisher-Yates with LCG.
        let mut lcg = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        for i in (1..n).rev() {
            lcg = lcg.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let j = (lcg as usize) % (i + 1);
            indices.swap(i, j);
        }
        indices
            .into_iter()
            .take(self.fan_out)
            .map(|i| candidates[i].clone())
            .collect()
    }
}
