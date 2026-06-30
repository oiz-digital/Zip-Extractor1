//! Peer reputation scoring for ZBX Chain P2P.
//!
//! ## Score model
//!
//! Each peer has a composite score in `[MIN_SCORE, MAX_SCORE]`.
//! Scores decay toward zero over time (favour recently active peers).
//!
//! ```text
//! score = latency_score
//!       + uptime_score
//!       + valid_message_bonus
//!       - penalties
//! ```
//!
//! ## Score thresholds
//!
//! | Score | Meaning |
//! |-------|---------|
//! | ≥ 80  | Excellent — prefer for sync |
//! | 20–79 | Good — normal operation |
//! | 1–19  | Marginal — deprioritised |
//! | ≤ 0   | Ban candidate |
//!
//! ## Penalties
//!
//! | Offence | Penalty |
//! |---------|---------|
//! | `InvalidMessage` | -10 |
//! | `SpamMessage` | -5 |
//! | `InvalidQC` | -30 |
//! | `UnknownBlock` | -2 |
//! | `TimeoutNoResponse` | -3 |
//! | `BadHandshake` | -50 (instant ban candidate) |

use crate::peer::PeerId;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tracing::{debug, warn};

/// Score ceiling.
pub const MAX_SCORE: i32 = 100;
/// Score floor (peer is banned when score reaches this).
pub const MIN_SCORE: i32 = -100;
/// Score at which a peer is banned.
pub const BAN_THRESHOLD: i32 = -50;
/// Score decay per interval.
pub const DECAY_PER_INTERVAL: i32 = 1;
/// Decay interval (scores decay toward zero periodically).
pub const DECAY_INTERVAL: Duration = Duration::from_secs(60);
/// Starting score for new peers.
pub const INITIAL_SCORE: i32 = 50;

/// Reward granted for a valid, useful message.
pub const VALID_MESSAGE_REWARD: i32 = 1;
/// Reward for a block that extends the chain.
pub const VALID_BLOCK_REWARD: i32 = 3;
/// Reward for a valid QC contributing to consensus.
pub const VALID_QC_REWARD: i32 = 2;

/// Penalty types applied to peer scores.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScorePenalty {
    /// Received a message that failed validation.
    InvalidMessage,
    /// Peer is sending too many messages too fast.
    SpamMessage,
    /// Received a proposal/vote with an invalid QC signature.
    InvalidQC,
    /// Peer requested a block we do not have (excessive unknown-block requests).
    UnknownBlock,
    /// Peer did not respond to a ping within the expected window.
    TimeoutNoResponse,
    /// Peer failed the Noise XX handshake.
    BadHandshake,
}

impl ScorePenalty {
    pub fn value(self) -> i32 {
        match self {
            ScorePenalty::InvalidMessage    => -10,
            ScorePenalty::SpamMessage       => -5,
            ScorePenalty::InvalidQC         => -30,
            ScorePenalty::UnknownBlock      => -2,
            ScorePenalty::TimeoutNoResponse => -3,
            ScorePenalty::BadHandshake      => -50,
        }
    }
}

/// Score components for one peer.
#[derive(Debug, Clone)]
pub struct PeerScore {
    /// Composite score.
    pub score:       i32,
    /// Cumulative valid messages contributed.
    pub valid_msgs:  u64,
    /// Cumulative invalid messages received.
    pub invalid_msgs: u64,
    /// Best (lowest) observed latency in ms.
    pub best_latency_ms: u32,
    /// Connection uptime in seconds.
    pub uptime_secs: u64,
    /// Whether this peer is currently banned.
    pub banned:      bool,
    /// Timestamp of the last score change.
    pub last_update: Instant,
    /// Timestamp of the last decay.
    last_decay:      Instant,
}

impl PeerScore {
    pub fn new() -> Self {
        PeerScore {
            score:          INITIAL_SCORE,
            valid_msgs:     0,
            invalid_msgs:   0,
            best_latency_ms: u32::MAX,
            uptime_secs:    0,
            banned:         false,
            last_update:    Instant::now(),
            last_decay:     Instant::now(),
        }
    }

    /// Clamp score within [MIN_SCORE, MAX_SCORE].
    fn clamp(&mut self) {
        self.score = self.score.clamp(MIN_SCORE, MAX_SCORE);
    }

    /// Apply a penalty and return `true` if the peer should now be banned.
    pub fn apply_penalty(&mut self, penalty: ScorePenalty) -> bool {
        self.score += penalty.value();
        self.invalid_msgs += 1;
        self.clamp();
        self.last_update = Instant::now();
        self.score <= BAN_THRESHOLD
    }

    /// Apply a positive reward.
    pub fn apply_reward(&mut self, reward: i32) {
        self.score += reward;
        self.valid_msgs += 1;
        self.clamp();
        self.last_update = Instant::now();
    }

    /// Decay score toward zero if the decay interval has passed.
    pub fn decay(&mut self) {
        let elapsed = self.last_decay.elapsed();
        if elapsed >= DECAY_INTERVAL {
            let intervals = (elapsed.as_secs() / DECAY_INTERVAL.as_secs()) as i32;
            let decay = intervals * DECAY_PER_INTERVAL;
            if self.score > 0 {
                self.score = (self.score - decay).max(0);
            } else if self.score < 0 {
                self.score = (self.score + decay).min(0);
            }
            self.last_decay = Instant::now();
        }
    }

    /// Update latency. Lower is better.
    pub fn update_latency(&mut self, latency_ms: u32) {
        if latency_ms < self.best_latency_ms {
            self.best_latency_ms = latency_ms;
            // Reward for low latency
            let bonus = if latency_ms < 50 { 2 } else if latency_ms < 200 { 1 } else { 0 };
            self.score = (self.score + bonus).min(MAX_SCORE);
        }
    }

    /// Update uptime (called periodically by the connection manager).
    pub fn tick_uptime(&mut self, delta_secs: u64) {
        self.uptime_secs += delta_secs;
        // Slight bonus for long-running connections (stable peers)
        if self.uptime_secs % 3600 == 0 && self.uptime_secs > 0 {
            self.score = (self.score + 1).min(MAX_SCORE);
        }
    }

    pub fn score_label(&self) -> &'static str {
        match self.score {
            s if s >= 80 => "excellent",
            20..=79      => "good",
            1..=19       => "marginal",
            _            => "ban-candidate",
        }
    }
}

impl Default for PeerScore {
    fn default() -> Self { Self::new() }
}

/// Manages peer scores for all connected peers.
pub struct PeerScorer {
    scores:  HashMap<PeerId, PeerScore>,
    /// Banned peer IDs (preserved for reconnection prevention).
    banned:  HashMap<PeerId, BanRecord>,
}

#[derive(Debug, Clone)]
pub struct BanRecord {
    pub reason:     String,
    pub banned_at:  std::time::SystemTime,
    pub final_score: i32,
}

impl PeerScorer {
    pub fn new() -> Self {
        PeerScorer {
            scores: HashMap::new(),
            banned: HashMap::new(),
        }
    }

    /// Add a new peer (starts at INITIAL_SCORE).
    pub fn add_peer(&mut self, id: PeerId) {
        self.scores.entry(id).or_insert_with(PeerScore::new);
    }

    /// Remove a peer on clean disconnect.
    pub fn remove_peer(&mut self, id: &PeerId) {
        self.scores.remove(id);
    }

    /// Apply a penalty to a peer. Returns `true` if the peer should be banned.
    pub fn penalise(&mut self, id: &PeerId, penalty: ScorePenalty) -> bool {
        let should_ban = if let Some(score) = self.scores.get_mut(id) {
            let ban = score.apply_penalty(penalty);
            if ban {
                warn!(
                    peer = ?id,
                    penalty = ?penalty,
                    score = score.score,
                    "peer hit ban threshold"
                );
            } else {
                debug!(
                    peer = ?id,
                    penalty = ?penalty,
                    score = score.score,
                    "peer penalised"
                );
            }
            ban
        } else {
            false
        };

        if should_ban {
            self.ban(id, format!("{:?}", penalty));
        }
        should_ban
    }

    /// Reward a peer for a useful contribution.
    pub fn reward(&mut self, id: &PeerId, reward: i32) {
        if let Some(score) = self.scores.get_mut(id) {
            score.apply_reward(reward);
        }
    }

    /// Update peer latency.
    pub fn update_latency(&mut self, id: &PeerId, latency_ms: u32) {
        if let Some(score) = self.scores.get_mut(id) {
            score.update_latency(latency_ms);
        }
    }

    /// Ban a peer directly (e.g. for bad handshake before score tracking).
    pub fn ban(&mut self, id: &PeerId, reason: String) {
        if let Some(mut score) = self.scores.remove(id) {
            score.banned = true;
            warn!(peer = ?id, reason, final_score = score.score, "peer banned");
            self.banned.insert(id.clone(), BanRecord {
                reason,
                banned_at:   std::time::SystemTime::now(),
                final_score: score.score,
            });
        }
    }

    pub fn is_banned(&self, id: &PeerId) -> bool {
        self.banned.contains_key(id)
    }

    /// Decay all peer scores (call periodically, e.g. every 60s).
    pub fn decay_all(&mut self) {
        for score in self.scores.values_mut() {
            score.decay();
        }
    }

    /// Get the score for a peer.
    pub fn get(&self, id: &PeerId) -> Option<&PeerScore> {
        self.scores.get(id)
    }

    /// Return peers sorted by score descending (best peers first).
    pub fn ranked_peers(&self) -> Vec<(&PeerId, &PeerScore)> {
        let mut v: Vec<_> = self.scores.iter().collect();
        v.sort_by(|a, b| b.1.score.cmp(&a.1.score));
        v
    }

    /// Return only peers with score above the given threshold.
    pub fn peers_above(&self, threshold: i32) -> Vec<&PeerId> {
        self.scores.iter()
            .filter(|(_, s)| s.score >= threshold)
            .map(|(id, _)| id)
            .collect()
    }

    /// Return the best `n` peers by score.
    pub fn best_peers(&self, n: usize) -> Vec<PeerId> {
        let mut ranked = self.ranked_peers();
        ranked.truncate(n);
        ranked.into_iter().map(|(id, _)| id.clone()).collect()
    }
}

impl Default for PeerScorer {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(b: u8) -> PeerId { PeerId([b; 32]) }

    #[test]
    fn initial_score() {
        let s = PeerScore::new();
        assert_eq!(s.score, INITIAL_SCORE);
        assert!(!s.banned);
    }

    #[test]
    fn penalty_reduces_score() {
        let mut s = PeerScore::new();
        s.apply_penalty(ScorePenalty::InvalidMessage);
        assert_eq!(s.score, INITIAL_SCORE + ScorePenalty::InvalidMessage.value());
    }

    #[test]
    fn score_clamped_to_max() {
        let mut s = PeerScore::new();
        s.score = MAX_SCORE;
        s.apply_reward(100);
        assert_eq!(s.score, MAX_SCORE);
    }

    #[test]
    fn score_clamped_to_min() {
        let mut s = PeerScore::new();
        s.score = MIN_SCORE;
        s.apply_penalty(ScorePenalty::BadHandshake);
        assert_eq!(s.score, MIN_SCORE);
    }

    #[test]
    fn bad_handshake_triggers_ban() {
        let mut scorer = PeerScorer::new();
        let id = peer(1);
        scorer.add_peer(id.clone());
        // Score starts at 50 — bad handshake is -50 → 0, then another one pushes below threshold
        scorer.penalise(&id, ScorePenalty::BadHandshake);
        let banned = scorer.penalise(&id, ScorePenalty::BadHandshake);
        // After two -50 penalties from 50: 50-50=0, 0-50=-50 ≤ BAN_THRESHOLD
        assert!(banned || scorer.is_banned(&id));
    }

    #[test]
    fn banned_peer_not_in_scores() {
        let mut scorer = PeerScorer::new();
        let id = peer(2);
        scorer.add_peer(id.clone());
        scorer.ban(&id, "test".to_string());
        assert!(scorer.is_banned(&id));
        assert!(scorer.get(&id).is_none());
    }

    #[test]
    fn ranked_peers_descending() {
        let mut scorer = PeerScorer::new();
        for i in 1u8..=5 {
            scorer.add_peer(peer(i));
            scorer.scores.get_mut(&peer(i)).unwrap().score = (i as i32) * 10;
        }
        let ranked = scorer.ranked_peers();
        assert_eq!(ranked[0].1.score, 50);
        assert_eq!(ranked[4].1.score, 10);
    }

    #[test]
    fn best_peers_returns_top_n() {
        let mut scorer = PeerScorer::new();
        for i in 1u8..=10 {
            scorer.add_peer(peer(i));
            scorer.scores.get_mut(&peer(i)).unwrap().score = i as i32;
        }
        let best = scorer.best_peers(3);
        assert_eq!(best.len(), 3);
    }

    #[test]
    fn latency_update_rewards_low_latency() {
        let mut scorer = PeerScorer::new();
        let id = peer(5);
        scorer.add_peer(id.clone());
        let before = scorer.get(&id).unwrap().score;
        scorer.update_latency(&id, 20); // <50ms → bonus 2
        let after = scorer.get(&id).unwrap().score;
        assert!(after >= before);
    }

    #[test]
    fn score_label_correct() {
        let mut s = PeerScore::new();
        s.score = 90; assert_eq!(s.score_label(), "excellent");
        s.score = 50; assert_eq!(s.score_label(), "good");
        s.score = 10; assert_eq!(s.score_label(), "marginal");
        s.score = 0;  assert_eq!(s.score_label(), "ban-candidate");
        s.score = -1; assert_eq!(s.score_label(), "ban-candidate");
    }
}
