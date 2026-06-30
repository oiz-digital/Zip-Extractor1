//! Peer-network policy types — reputation, ban-list, eclipse-resistance.
//!
//! Closes Item #3 (network security): "no peer-reputation system, no
//! eclipse-attack mitigation". This module provides the canonical types
//! that `zbx-net` will consume; it does NOT itself open sockets.
//!
//! ## Reputation model
//!
//! Each peer accumulates a signed `i32` score in `[-1024, +1024]`. Specific
//! protocol events (good block, invalid signature, ping timeout, …) map
//! to a fixed score delta via `ReputationDelta::for_event`. Scores
//! decay 1/2 per `decay_period_secs` so transient bad behaviour fades.
//!
//! ## Eclipse mitigation
//!
//! `PeerPolicy` enforces:
//! - Per-`/16` IPv4 subnet cap (`max_peers_per_subnet`) so a single
//!   network operator cannot fill the inbound table.
//! - Hard `min_outbound` floor — node refuses to drop the last outbound
//!   peer slot under inbound pressure.
//! - `bootstrap_pin_count` — N anchor peers cannot be evicted.
//!
//! ## Discipline
//! - All collections are `BTreeMap`/`BTreeSet` for canonical RLP ordering.
//! - `validate()` runs in BOTH constructor AND `Decodable::decode`.
//! - Newtype `Encodable` impls use `s.append(&self.inner.as_ref())`
//!   when inner is an array (LESSON #11 caveat).

use std::collections::{BTreeMap, BTreeSet};

use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// PeerId — fixed 32-byte node identifier (typically Ed25519 pubkey).
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct PeerId(pub [u8; 32]);

impl PeerId {
    pub const fn zero() -> Self { Self([0u8; 32]) }
    pub fn as_bytes(&self) -> &[u8; 32] { &self.0 }
}

impl Encodable for PeerId {
    fn rlp_append(&self, s: &mut RlpStream) {
        // LESSON #11 strict form: direct delegation (not `s.append(&inner)`)
        // because PeerId is a newtype and is itself wrapped in an outer
        // `s.append(&peer_id)`. Wrapping again would double-count the slot
        // unless the inner happened to close the parent list — non-portable.
        let slice: &[u8] = self.0.as_ref();
        slice.rlp_append(s);
    }
}

impl Decodable for PeerId {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        let bytes: Vec<u8> = rlp.as_val()?;
        if bytes.len() != 32 {
            return Err(DecoderError::Custom("PeerId must be 32 bytes"));
        }
        let mut out = [0u8; 32];
        out.copy_from_slice(&bytes);
        Ok(Self(out))
    }
}

// ---------------------------------------------------------------------------
// PeerEvent — wire-protocol-observable behaviour worth scoring.
// Tag bytes are stable on disk — DO NOT renumber.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum PeerEvent {
    /// Peer relayed a valid, fresh block within its slot.
    GoodBlock         = 0,
    /// Peer answered a getblock/getheader request promptly.
    GoodResponse      = 1,
    /// Peer relayed a transaction we accepted into our mempool.
    GoodTx            = 2,
    /// Peer relayed a block whose proposer signature failed.
    InvalidBlock      = 10,
    /// Peer sent a transaction with bad signature/nonce/etc.
    InvalidTx         = 11,
    /// Peer sent malformed protocol bytes that failed to decode.
    MalformedMessage  = 12,
    /// Peer relayed an already-seen message inside the dedup window.
    SpamDuplicate     = 13,
    /// Peer ignored a request past the timeout deadline.
    Timeout           = 14,
    /// Peer sent a block on a fork at a height we already finalized.
    EquivocationFork  = 20,
}

impl PeerEvent {
    pub fn tag(self) -> u8 { self as u8 }

    pub fn from_tag(t: u8) -> Result<Self, DecoderError> {
        match t {
            0  => Ok(Self::GoodBlock),
            1  => Ok(Self::GoodResponse),
            2  => Ok(Self::GoodTx),
            10 => Ok(Self::InvalidBlock),
            11 => Ok(Self::InvalidTx),
            12 => Ok(Self::MalformedMessage),
            13 => Ok(Self::SpamDuplicate),
            14 => Ok(Self::Timeout),
            20 => Ok(Self::EquivocationFork),
            _  => Err(DecoderError::Custom("invalid PeerEvent tag")),
        }
    }
}

impl Encodable for PeerEvent {
    fn rlp_append(&self, s: &mut RlpStream) { self.tag().rlp_append(s); }
}

impl Decodable for PeerEvent {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        Self::from_tag(rlp.as_val()?)
    }
}

// ---------------------------------------------------------------------------
// ReputationDelta — fixed per-event score delta. Tuned for the
// "12 invalid blocks → ban" rule (12 × −10 = −120 < ban_threshold = −100).
// ---------------------------------------------------------------------------

pub struct ReputationDelta;

impl ReputationDelta {
    pub fn for_event(e: PeerEvent) -> i32 {
        match e {
            PeerEvent::GoodBlock        => 5,
            PeerEvent::GoodResponse     => 1,
            PeerEvent::GoodTx           => 1,
            PeerEvent::InvalidBlock     => -10,
            PeerEvent::InvalidTx        => -2,
            PeerEvent::MalformedMessage => -5,
            PeerEvent::SpamDuplicate    => -1,
            PeerEvent::Timeout          => -3,
            PeerEvent::EquivocationFork => -200,   // single occurrence → ban
        }
    }
}

// ---------------------------------------------------------------------------
// PeerScore — clamped, signed integer reputation.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PeerScore(pub i32);

impl PeerScore {
    pub const MIN: i32 = -1024;
    pub const MAX: i32 =  1024;

    pub fn new(v: i32) -> Self {
        Self(v.clamp(Self::MIN, Self::MAX))
    }

    pub fn apply(self, delta: i32) -> Self {
        Self::new(self.0.saturating_add(delta))
    }

    /// Halve toward zero (each `decay_period_secs`).
    pub fn decay(self) -> Self { Self(self.0 / 2) }
}

impl Default for PeerScore {
    fn default() -> Self { Self(0) }
}

// `rlp::Encodable` is NOT implemented for `i32`. We bit-cast through `u32`
// (round-trip preserves all bits including the sign bit). Direct delegation
// per LESSON #11 strict form — see PeerId for rationale.
impl Encodable for PeerScore {
    fn rlp_append(&self, s: &mut RlpStream) { (self.0 as u32).rlp_append(s); }
}

impl Decodable for PeerScore {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        let raw: u32 = rlp.as_val()?;
        Ok(Self::new(raw as i32))
    }
}

// ---------------------------------------------------------------------------
// PeerPolicy — eclipse-resistance configuration consumed by zbx-net.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PeerPolicy {
    pub max_inbound:           u16,
    pub max_outbound:          u16,
    pub min_outbound:          u16,
    pub max_peers_per_subnet:  u16,   // per IPv4 /16
    pub bootstrap_pin_count:   u16,
    pub ban_threshold:         i32,   // score ≤ this → banned
    pub ban_duration_secs:     u64,
    pub decay_period_secs:     u64,
}

impl PeerPolicy {
    pub fn mainnet_default() -> Self {
        Self {
            max_inbound:          80,
            max_outbound:         16,
            min_outbound:         8,
            max_peers_per_subnet: 4,
            bootstrap_pin_count:  4,
            ban_threshold:        -100,
            ban_duration_secs:    24 * 60 * 60,
            decay_period_secs:    60 * 60,
        }
    }

    pub fn validate(&self) -> Result<(), DecoderError> {
        if self.min_outbound > self.max_outbound {
            return Err(DecoderError::Custom("PeerPolicy: min_outbound > max_outbound"));
        }
        if self.max_outbound == 0 {
            return Err(DecoderError::Custom("PeerPolicy: max_outbound = 0"));
        }
        if self.bootstrap_pin_count > self.max_outbound {
            return Err(DecoderError::Custom("PeerPolicy: bootstrap_pin_count > max_outbound"));
        }
        if self.max_peers_per_subnet == 0 {
            return Err(DecoderError::Custom("PeerPolicy: max_peers_per_subnet = 0"));
        }
        if self.ban_threshold >= 0 {
            return Err(DecoderError::Custom("PeerPolicy: ban_threshold must be negative"));
        }
        if self.ban_duration_secs == 0 {
            return Err(DecoderError::Custom("PeerPolicy: ban_duration_secs = 0"));
        }
        if self.decay_period_secs == 0 {
            return Err(DecoderError::Custom("PeerPolicy: decay_period_secs = 0"));
        }
        Ok(())
    }
}

impl Encodable for PeerPolicy {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(8);
        s.append(&self.max_inbound);
        s.append(&self.max_outbound);
        s.append(&self.min_outbound);
        s.append(&self.max_peers_per_subnet);
        s.append(&self.bootstrap_pin_count);
        // Bit-cast i32 → u32 (rlp crate has no native i32 impl).
        s.append(&(self.ban_threshold as u32));
        s.append(&self.ban_duration_secs);
        s.append(&self.decay_period_secs);
    }
}

impl Decodable for PeerPolicy {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if rlp.item_count()? != 8 { return Err(DecoderError::RlpIncorrectListLen); }
        let p = Self {
            max_inbound:          rlp.val_at(0)?,
            max_outbound:         rlp.val_at(1)?,
            min_outbound:         rlp.val_at(2)?,
            max_peers_per_subnet: rlp.val_at(3)?,
            bootstrap_pin_count:  rlp.val_at(4)?,
            ban_threshold:        { let raw: u32 = rlp.val_at(5)?; raw as i32 },
            ban_duration_secs:    rlp.val_at(6)?,
            decay_period_secs:    rlp.val_at(7)?,
        };
        p.validate()?;
        Ok(p)
    }
}

// ---------------------------------------------------------------------------
// PeerRecord — per-peer persisted state (score, ban window, last-seen).
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PeerRecord {
    pub peer:           PeerId,
    pub score:          PeerScore,
    pub last_seen_unix: u64,
    pub banned_until:   u64,    // 0 = not banned
}

impl PeerRecord {
    pub fn new(peer: PeerId) -> Self {
        Self { peer, score: PeerScore::default(), last_seen_unix: 0, banned_until: 0 }
    }

    pub fn is_banned_at(&self, now_unix: u64) -> bool {
        self.banned_until > now_unix
    }
}

impl Encodable for PeerRecord {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(4);
        s.append(&self.peer);
        s.append(&self.score);
        s.append(&self.last_seen_unix);
        s.append(&self.banned_until);
    }
}

impl Decodable for PeerRecord {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if rlp.item_count()? != 4 { return Err(DecoderError::RlpIncorrectListLen); }
        Ok(Self {
            peer:           rlp.val_at(0)?,
            score:          rlp.val_at(1)?,
            last_seen_unix: rlp.val_at(2)?,
            banned_until:   rlp.val_at(3)?,
        })
    }
}

// ---------------------------------------------------------------------------
// PeerBook — in-memory + RLP-encodable peer registry. Pure logic, no I/O.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PeerBook {
    pub policy:    PeerPolicy,
    pub records:   BTreeMap<PeerId, PeerRecord>,
    pub bootstrap: BTreeSet<PeerId>,
}

impl PeerBook {
    pub fn new(policy: PeerPolicy) -> Result<Self, DecoderError> {
        policy.validate()?;
        Ok(Self { policy, records: BTreeMap::new(), bootstrap: BTreeSet::new() })
    }

    pub fn validate(&self) -> Result<(), DecoderError> {
        self.policy.validate()?;
        if self.bootstrap.len() > self.policy.bootstrap_pin_count as usize {
            return Err(DecoderError::Custom("PeerBook: too many bootstrap pins"));
        }
        for (pid, rec) in &self.records {
            if &rec.peer != pid {
                return Err(DecoderError::Custom("PeerBook: record/key peer mismatch"));
            }
        }
        Ok(())
    }

    /// Apply an event delta to a peer; auto-bans if the score crosses
    /// `policy.ban_threshold`. Returns the new score.
    pub fn observe(&mut self, peer: PeerId, ev: PeerEvent, now_unix: u64) -> i32 {
        let delta = ReputationDelta::for_event(ev);
        let rec = self.records.entry(peer).or_insert_with(|| PeerRecord::new(peer));
        rec.score = rec.score.apply(delta);
        rec.last_seen_unix = now_unix;
        if rec.score.0 <= self.policy.ban_threshold {
            rec.banned_until = now_unix.saturating_add(self.policy.ban_duration_secs);
        }
        rec.score.0
    }

    /// Check whether this peer is currently banned at `now_unix`. Bootstrap
    /// pins are NEVER banned (operator override) — this matches Bitcoin
    /// Core's `-addnode` semantics.
    pub fn is_banned(&self, peer: &PeerId, now_unix: u64) -> bool {
        if self.bootstrap.contains(peer) { return false; }
        self.records.get(peer).map(|r| r.is_banned_at(now_unix)).unwrap_or(false)
    }

    /// Decay every peer's score by half. Call on every `decay_period_secs`.
    pub fn decay_all(&mut self) {
        for r in self.records.values_mut() {
            r.score = r.score.decay();
        }
    }

    pub fn pin_bootstrap(&mut self, peer: PeerId) -> Result<(), DecoderError> {
        if self.bootstrap.len() >= self.policy.bootstrap_pin_count as usize {
            return Err(DecoderError::Custom("PeerBook: bootstrap_pin_count exceeded"));
        }
        self.bootstrap.insert(peer);
        Ok(())
    }
}

impl Encodable for PeerBook {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(3);
        s.append(&self.policy);
        s.begin_list(self.records.len());
        for r in self.records.values() { s.append(r); }
        s.begin_list(self.bootstrap.len());
        for p in &self.bootstrap { s.append(p); }
    }
}

impl Decodable for PeerBook {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if rlp.item_count()? != 3 { return Err(DecoderError::RlpIncorrectListLen); }
        let policy: PeerPolicy = rlp.val_at(0)?;

        let recs_rlp = rlp.at(1)?;
        let mut records = BTreeMap::new();
        for i in 0..recs_rlp.item_count()? {
            let r: PeerRecord = recs_rlp.val_at(i)?;
            records.insert(r.peer, r);
        }

        let pins_rlp = rlp.at(2)?;
        let mut bootstrap = BTreeSet::new();
        for i in 0..pins_rlp.item_count()? {
            let p: PeerId = pins_rlp.val_at(i)?;
            bootstrap.insert(p);
        }

        let book = Self { policy, records, bootstrap };
        book.validate()?;
        Ok(book)
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn pid(b: u8) -> PeerId {
        let mut a = [0u8; 32];
        a[31] = b;
        PeerId(a)
    }

    #[test]
    fn score_clamps_at_bounds() {
        assert_eq!(PeerScore::new(99_999).0, PeerScore::MAX);
        assert_eq!(PeerScore::new(-99_999).0, PeerScore::MIN);
        assert_eq!(PeerScore::new(0).apply(10).0, 10);
        assert_eq!(PeerScore::new(1024).apply(10).0, 1024);
    }

    #[test]
    fn score_decay_halves_toward_zero() {
        assert_eq!(PeerScore::new(100).decay().0, 50);
        assert_eq!(PeerScore::new(-100).decay().0, -50);
        assert_eq!(PeerScore::new(1).decay().0, 0);
    }

    #[test]
    fn event_tag_roundtrip() {
        for ev in [
            PeerEvent::GoodBlock, PeerEvent::GoodResponse, PeerEvent::GoodTx,
            PeerEvent::InvalidBlock, PeerEvent::InvalidTx, PeerEvent::MalformedMessage,
            PeerEvent::SpamDuplicate, PeerEvent::Timeout, PeerEvent::EquivocationFork,
        ] {
            assert_eq!(PeerEvent::from_tag(ev.tag()).unwrap(), ev);
        }
        assert!(PeerEvent::from_tag(99).is_err());
    }

    #[test]
    fn delta_is_negative_for_bad_events() {
        assert!(ReputationDelta::for_event(PeerEvent::InvalidBlock) < 0);
        assert!(ReputationDelta::for_event(PeerEvent::EquivocationFork) <= -100);
        assert!(ReputationDelta::for_event(PeerEvent::GoodBlock) > 0);
    }

    #[test]
    fn policy_default_validates() {
        PeerPolicy::mainnet_default().validate().unwrap();
    }

    #[test]
    fn policy_rejects_invalid_thresholds() {
        let mut p = PeerPolicy::mainnet_default();
        p.ban_threshold = 0;
        assert!(p.validate().is_err());
        let mut p = PeerPolicy::mainnet_default();
        p.min_outbound = p.max_outbound + 1;
        assert!(p.validate().is_err());
        let mut p = PeerPolicy::mainnet_default();
        p.bootstrap_pin_count = p.max_outbound + 1;
        assert!(p.validate().is_err());
        let mut p = PeerPolicy::mainnet_default();
        p.max_peers_per_subnet = 0;
        assert!(p.validate().is_err());
    }

    #[test]
    fn observe_updates_score_and_last_seen() {
        let mut book = PeerBook::new(PeerPolicy::mainnet_default()).unwrap();
        let s = book.observe(pid(1), PeerEvent::GoodBlock, 100);
        assert_eq!(s, 5);
        assert_eq!(book.records[&pid(1)].last_seen_unix, 100);
        assert!(!book.is_banned(&pid(1), 200));
    }

    #[test]
    fn twelve_invalid_blocks_trigger_ban() {
        let mut book = PeerBook::new(PeerPolicy::mainnet_default()).unwrap();
        // 12 × −10 = −120 < ban_threshold (−100).
        for _ in 0..12 {
            book.observe(pid(1), PeerEvent::InvalidBlock, 1000);
        }
        assert!(book.is_banned(&pid(1), 1000));
        assert!(book.is_banned(&pid(1), 1000 + 60));
        // Past the ban window, no longer banned.
        assert!(!book.is_banned(&pid(1), 1000 + 86_401));
    }

    #[test]
    fn equivocation_one_shot_ban() {
        let mut book = PeerBook::new(PeerPolicy::mainnet_default()).unwrap();
        book.observe(pid(1), PeerEvent::EquivocationFork, 1000);
        assert!(book.is_banned(&pid(1), 1000));
    }

    #[test]
    fn bootstrap_pin_immune_to_ban() {
        let mut book = PeerBook::new(PeerPolicy::mainnet_default()).unwrap();
        book.pin_bootstrap(pid(1)).unwrap();
        book.observe(pid(1), PeerEvent::EquivocationFork, 1000);
        // Score still drops, but bootstrap override returns is_banned=false.
        assert!(book.records[&pid(1)].score.0 <= -100);
        assert!(!book.is_banned(&pid(1), 1000));
    }

    #[test]
    fn pin_bootstrap_respects_count_cap() {
        let mut book = PeerBook::new(PeerPolicy::mainnet_default()).unwrap();
        // Default bootstrap_pin_count = 4.
        for i in 1..=4 { book.pin_bootstrap(pid(i)).unwrap(); }
        assert!(book.pin_bootstrap(pid(5)).is_err());
    }

    #[test]
    fn decay_all_halves_every_score() {
        let mut book = PeerBook::new(PeerPolicy::mainnet_default()).unwrap();
        book.observe(pid(1), PeerEvent::GoodBlock, 100);   // +5
        book.observe(pid(1), PeerEvent::GoodBlock, 100);   // +10
        book.observe(pid(2), PeerEvent::InvalidBlock, 100);// -10
        book.decay_all();
        assert_eq!(book.records[&pid(1)].score.0,  5);
        assert_eq!(book.records[&pid(2)].score.0, -5);
    }

    #[test]
    fn rlp_round_trip_book() {
        let mut book = PeerBook::new(PeerPolicy::mainnet_default()).unwrap();
        book.observe(pid(1), PeerEvent::GoodBlock, 100);
        book.observe(pid(2), PeerEvent::InvalidTx, 200);
        book.pin_bootstrap(pid(7)).unwrap();
        let enc = rlp::encode(&book);
        let dec: PeerBook = rlp::decode(&enc).unwrap();
        assert_eq!(book, dec);
    }

    #[test]
    fn rlp_round_trip_policy() {
        let p = PeerPolicy::mainnet_default();
        let enc = rlp::encode(&p);
        let dec: PeerPolicy = rlp::decode(&enc).unwrap();
        assert_eq!(p, dec);
    }

    #[test]
    fn rlp_decode_rejects_invalid_policy() {
        // Build a policy with ban_threshold = 0, encode, attempt decode.
        let mut p = PeerPolicy::mainnet_default();
        p.ban_threshold = 0;
        let mut s = RlpStream::new();
        s.begin_list(8);
        s.append(&p.max_inbound);
        s.append(&p.max_outbound);
        s.append(&p.min_outbound);
        s.append(&p.max_peers_per_subnet);
        s.append(&p.bootstrap_pin_count);
        s.append(&(p.ban_threshold as u32));
        s.append(&p.ban_duration_secs);
        s.append(&p.decay_period_secs);
        let enc = s.out();
        let r: Result<PeerPolicy, _> = rlp::decode(&enc);
        assert!(r.is_err());
    }
}
