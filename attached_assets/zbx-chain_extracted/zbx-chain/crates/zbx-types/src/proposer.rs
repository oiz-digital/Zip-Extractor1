//! Proposer selection — deterministic block proposer scheduling.
//!
//! Closes Item #2 (consensus security) sub-bullet
//! "deterministic proposer selection". The scheduler is agnostic to the
//! validator set's internal order: input is the canonical `ValidatorSet`,
//! a per-block `ProposerSeed` (H256, derived in zbx-consensus from the
//! previous block's RANDAO mix), and the target `height`.
//!
//! Three algorithms supported, every one is **bit-deterministic across
//! peers** — same `(seed, height, set)` triple always yields the same
//! `Address`:
//!
//! * `RoundRobin` — `validators[height % N]` after sorting by stake desc,
//!   address asc tiebreaker. No randomness; useful for small testnets.
//! * `WeightedRandom` — `seed → u64 → cumulative-stake bucket`. Higher
//!   stake = higher selection probability. Production default.
//! * `Fixed(addr)` — pinned proposer for the entire epoch. Devnet-only.
//!
//! Discipline:
//! - `BTreeMap<u64, Address>` for `ProposerSchedule` so RLP is canonical
//!   and `Eq` is stable.
//! - `validate()` runs in BOTH constructor AND `Decodable::decode`.
//! - `s.append(&inner)` inside `begin_list(N)` — never the naked
//!   `inner.rlp_append(s)`.
//! - Newtype `Encodable` impls use `self.inner.rlp_append(s)` for direct
//!   delegation (LESSON #11).

use std::collections::BTreeMap;

use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};
use serde::{Deserialize, Serialize};

use crate::address::Address;
use crate::consensus::{ValidatorInfo, ValidatorSet};
use crate::H256;

// ---------------------------------------------------------------------------
// ProposerSeed — newtype around H256 so type signatures self-document
// "this is the consensus randomness, not a generic hash".
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct ProposerSeed {
    pub inner: H256,
}

impl ProposerSeed {
    pub const fn new(h: H256) -> Self {
        Self { inner: h }
    }
}

impl Encodable for ProposerSeed {
    fn rlp_append(&self, s: &mut RlpStream) {
        // LESSON #11 strict form: direct delegation. `s.append(&inner)` here
        // would only work if Seed happens to close its parent list — this
        // breaks when Seed is used as a non-tail field of a composite type.
        let slice: &[u8] = self.inner.as_bytes();
        slice.rlp_append(s);
    }
}

impl Decodable for ProposerSeed {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        let bytes: Vec<u8> = rlp.as_val()?;
        if bytes.len() != 32 {
            return Err(DecoderError::Custom("ProposerSeed must be 32 bytes"));
        }
        Ok(Self { inner: H256::from_slice(&bytes) })
    }
}

// ---------------------------------------------------------------------------
// Local Address codec helpers (matches sibling-module discipline — see
// consensus.rs / governance.rs / execution.rs / slashing.rs).
// ---------------------------------------------------------------------------

fn append_address(s: &mut RlpStream, a: &Address) {
    s.append(&a.0.as_ref());
}

fn decode_address(rlp: &Rlp) -> Result<Address, DecoderError> {
    let bytes: Vec<u8> = rlp.as_val()?;
    if bytes.len() != 20 {
        return Err(DecoderError::Custom("address must be 20 bytes"));
    }
    let mut out = [0u8; 20];
    out.copy_from_slice(&bytes);
    Ok(Address(out))
}

// ---------------------------------------------------------------------------
// ProposerAlgorithm — selection strategy tag.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum ProposerAlgorithm {
    RoundRobin,
    WeightedRandom,
    Fixed,
}

impl ProposerAlgorithm {
    pub fn tag(self) -> u8 {
        match self {
            Self::RoundRobin     => 0,
            Self::WeightedRandom => 1,
            Self::Fixed          => 2,
        }
    }

    pub fn from_tag(t: u8) -> Result<Self, DecoderError> {
        match t {
            0 => Ok(Self::RoundRobin),
            1 => Ok(Self::WeightedRandom),
            2 => Ok(Self::Fixed),
            _ => Err(DecoderError::Custom("invalid ProposerAlgorithm tag")),
        }
    }
}

impl Encodable for ProposerAlgorithm {
    fn rlp_append(&self, s: &mut RlpStream) { self.tag().rlp_append(s); }
}

impl Decodable for ProposerAlgorithm {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        let t: u8 = rlp.as_val()?;
        Self::from_tag(t)
    }
}

// ---------------------------------------------------------------------------
// ProposerSchedule — pre-computed (height → proposer) map for an epoch.
// Used so peers can verify "block at height H proposed by V" without
// re-running the selection algorithm against the live validator set.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProposerSchedule {
    pub epoch_start: u64,
    pub epoch_end:   u64,                       // inclusive
    pub algorithm:   ProposerAlgorithm,
    pub assignments: BTreeMap<u64, Address>,    // height → proposer
}

impl ProposerSchedule {
    pub fn new(
        epoch_start: u64,
        epoch_end:   u64,
        algorithm:   ProposerAlgorithm,
        assignments: BTreeMap<u64, Address>,
    ) -> Result<Self, DecoderError> {
        let s = Self { epoch_start, epoch_end, algorithm, assignments };
        s.validate()?;
        Ok(s)
    }

    pub fn validate(&self) -> Result<(), DecoderError> {
        if self.epoch_start > self.epoch_end {
            return Err(DecoderError::Custom("epoch_start > epoch_end"));
        }
        for (&h, _) in &self.assignments {
            if h < self.epoch_start || h > self.epoch_end {
                return Err(DecoderError::Custom("assignment height out of epoch"));
            }
        }
        Ok(())
    }

    /// Look up the proposer for a given height. Returns `None` if either
    /// the height is outside the epoch or the algorithm chose to leave the
    /// slot unscheduled (legitimate — a `Fixed` schedule may be sparse).
    pub fn proposer_at(&self, height: u64) -> Option<Address> {
        self.assignments.get(&height).copied()
    }
}

impl Encodable for ProposerSchedule {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(4);
        s.append(&self.epoch_start);
        s.append(&self.epoch_end);
        s.append(&self.algorithm);
        s.begin_list(self.assignments.len());
        for (h, a) in &self.assignments {
            s.begin_list(2);
            s.append(h);
            append_address(s, a);
        }
    }
}

impl Decodable for ProposerSchedule {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if rlp.item_count()? != 4 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        let epoch_start: u64               = rlp.val_at(0)?;
        let epoch_end:   u64               = rlp.val_at(1)?;
        let algorithm:   ProposerAlgorithm = rlp.val_at(2)?;
        let pairs = rlp.at(3)?;

        let mut assignments = BTreeMap::new();
        for i in 0..pairs.item_count()? {
            let pair = pairs.at(i)?;
            if pair.item_count()? != 2 {
                return Err(DecoderError::RlpIncorrectListLen);
            }
            let h: u64 = pair.val_at(0)?;
            let a      = decode_address(&pair.at(1)?)?;
            assignments.insert(h, a);
        }

        let s = Self { epoch_start, epoch_end, algorithm, assignments };
        s.validate()?;
        Ok(s)
    }
}

// ---------------------------------------------------------------------------
// Selection — pure deterministic functions. No I/O, no randomness beyond
// the seed parameter.
// ---------------------------------------------------------------------------

/// Compute the proposer for `(height, seed)` against `set`.
/// Returns `None` if the set has no eligible (non-jailed) validators.
///
/// Determinism contract: every node calling this with the same arguments
/// MUST get the same `Address` — no float, no time, no thread state.
/// Jailed validators are excluded from the eligible pool by every algorithm.
pub fn select_proposer(
    algorithm: ProposerAlgorithm,
    set:       &ValidatorSet,
    seed:      ProposerSeed,
    height:    u64,
) -> Option<Address> {
    let eligible: Vec<&ValidatorInfo> = set
        .members()
        .values()
        .filter(|v| !v.jailed)
        .collect();
    if eligible.is_empty() {
        return None;
    }

    match algorithm {
        ProposerAlgorithm::RoundRobin => {
            // Sort by (voting_power desc, address asc) for stable ordering
            // across peers regardless of map iteration order.
            let mut sorted = eligible.clone();
            sorted.sort_by(|a, b| {
                b.voting_power.cmp(&a.voting_power).then_with(|| a.address.cmp(&b.address))
            });
            let idx = (height as usize) % sorted.len();
            Some(sorted[idx].address)
        }

        ProposerAlgorithm::WeightedRandom => {
            // Mix seed + height into a u128 so we get a deterministic but
            // height-varying draw from the cumulative-power distribution.
            let total_power: u128 = eligible.iter().map(|v| v.voting_power as u128).sum();
            if total_power == 0 {
                // Fall through to round-robin if nobody has voting power.
                return select_proposer(ProposerAlgorithm::RoundRobin, set, seed, height);
            }
            let mut buf = [0u8; 40];
            buf[..32].copy_from_slice(seed.inner.as_bytes());
            buf[32..].copy_from_slice(&height.to_be_bytes());
            let mix = blake_u128(&buf);
            let pick = mix % total_power;

            let mut sorted = eligible.clone();
            sorted.sort_by(|a, b| a.address.cmp(&b.address));
            let mut acc: u128 = 0;
            for v in sorted {
                acc += v.voting_power as u128;
                if pick < acc {
                    return Some(v.address);
                }
            }
            // Unreachable given total_power > 0, but stay defensive.
            None
        }

        ProposerAlgorithm::Fixed => {
            // For Fixed, we take the lexicographically smallest address.
            // Devnet-only — production should never set this.
            let mut sorted = eligible.clone();
            sorted.sort_by(|a, b| a.address.cmp(&b.address));
            sorted.first().map(|v| v.address)
        }
    }
}

/// Tiny in-place hash → u128, no extra deps. We fold the input into a
/// 128-bit accumulator using FNV-1a-style mixing. Determinism is the
/// only requirement — collision resistance is irrelevant because the
/// caller already mixed a 256-bit seed in.
fn blake_u128(bytes: &[u8]) -> u128 {
    const FNV_OFFSET: u128 = 0x6c62272e07bb014262b821756295c58d;
    const FNV_PRIME:  u128 = 0x0000000001000000000000000000013b;
    let mut h = FNV_OFFSET;
    for &b in bytes {
        h ^= b as u128;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::ValidatorInfo;

    fn addr(byte: u8) -> Address {
        let mut a = [0u8; 20];
        a[19] = byte;
        Address(a)
    }

    fn three_validator_set() -> ValidatorSet {
        let mut s = ValidatorSet::new();
        s.upsert(ValidatorInfo::new(addr(1), 100, false, 0).unwrap()).unwrap();
        s.upsert(ValidatorInfo::new(addr(2), 200, false, 0).unwrap()).unwrap();
        s.upsert(ValidatorInfo::new(addr(3), 300, false, 0).unwrap()).unwrap();
        s
    }

    #[test]
    fn round_robin_is_deterministic_and_cycles() {
        let set = three_validator_set();
        let seed = ProposerSeed::new(H256::zero());
        // Sort order is (voting_power desc, addr asc): addr(3)=300, addr(2)=200, addr(1)=100
        let p0 = select_proposer(ProposerAlgorithm::RoundRobin, &set, seed, 0).unwrap();
        let p1 = select_proposer(ProposerAlgorithm::RoundRobin, &set, seed, 1).unwrap();
        let p2 = select_proposer(ProposerAlgorithm::RoundRobin, &set, seed, 2).unwrap();
        let p3 = select_proposer(ProposerAlgorithm::RoundRobin, &set, seed, 3).unwrap();
        assert_eq!(p0, addr(3));
        assert_eq!(p1, addr(2));
        assert_eq!(p2, addr(1));
        assert_eq!(p3, addr(3)); // wraps
    }

    #[test]
    fn weighted_random_respects_stake() {
        let set = three_validator_set();
        // Sample 1000 heights with a fixed seed; addr(3) has 50% power so
        // should win the most. Deterministic so this is not flaky.
        let seed = ProposerSeed::new(H256::repeat_byte(0xab));
        let mut counts = [0u32; 4]; // index 1..=3
        for h in 0..1000u64 {
            let p = select_proposer(ProposerAlgorithm::WeightedRandom, &set, seed, h).unwrap();
            counts[p.as_bytes()[19] as usize] += 1;
        }
        // addr(3) should win the most (300/600 power).
        assert!(counts[3] > counts[2]);
        assert!(counts[2] > counts[1]);
        // Total adds to 1000.
        assert_eq!(counts[1] + counts[2] + counts[3], 1000);
    }

    #[test]
    fn weighted_random_is_deterministic() {
        let set = three_validator_set();
        let seed = ProposerSeed::new(H256::repeat_byte(0x42));
        let a = select_proposer(ProposerAlgorithm::WeightedRandom, &set, seed, 12345);
        let b = select_proposer(ProposerAlgorithm::WeightedRandom, &set, seed, 12345);
        assert_eq!(a, b);
    }

    #[test]
    fn fixed_returns_lowest_address() {
        let set = three_validator_set();
        let seed = ProposerSeed::new(H256::zero());
        for h in 0..10 {
            let p = select_proposer(ProposerAlgorithm::Fixed, &set, seed, h).unwrap();
            assert_eq!(p, addr(1));
        }
    }

    #[test]
    fn empty_set_returns_none() {
        let set = ValidatorSet::new();
        let seed = ProposerSeed::new(H256::zero());
        assert!(select_proposer(ProposerAlgorithm::RoundRobin, &set, seed, 0).is_none());
        assert!(select_proposer(ProposerAlgorithm::WeightedRandom, &set, seed, 0).is_none());
    }

    #[test]
    fn jailed_validators_excluded() {
        let mut set = ValidatorSet::new();
        set.upsert(ValidatorInfo::new(addr(1), 100, true,  100).unwrap()).unwrap(); // jailed
        set.upsert(ValidatorInfo::new(addr(2), 200, false, 0).unwrap()).unwrap();
        let seed = ProposerSeed::new(H256::zero());
        // Round-robin only sees addr(2), regardless of height.
        for h in 0..5 {
            assert_eq!(select_proposer(ProposerAlgorithm::RoundRobin, &set, seed, h), Some(addr(2)));
        }
    }

    #[test]
    fn schedule_validates_height_bounds() {
        let mut a = BTreeMap::new();
        a.insert(5, addr(1));
        a.insert(15, addr(2)); // out of bounds
        let r = ProposerSchedule::new(0, 10, ProposerAlgorithm::RoundRobin, a);
        assert!(r.is_err());
    }

    #[test]
    fn schedule_rejects_inverted_epoch() {
        let r = ProposerSchedule::new(10, 5, ProposerAlgorithm::RoundRobin, BTreeMap::new());
        assert!(r.is_err());
    }

    #[test]
    fn rlp_round_trip_schedule() {
        let mut a = BTreeMap::new();
        a.insert(0, addr(1));
        a.insert(1, addr(2));
        a.insert(2, addr(3));
        let s = ProposerSchedule::new(0, 10, ProposerAlgorithm::WeightedRandom, a).unwrap();
        let enc = rlp::encode(&s);
        let dec: ProposerSchedule = rlp::decode(&enc).unwrap();
        assert_eq!(s, dec);
    }

    #[test]
    fn rlp_round_trip_seed() {
        let s = ProposerSeed::new(H256::repeat_byte(0xff));
        let enc = rlp::encode(&s);
        let dec: ProposerSeed = rlp::decode(&enc).unwrap();
        assert_eq!(s, dec);
    }

    #[test]
    fn algorithm_tag_round_trip() {
        for a in [
            ProposerAlgorithm::RoundRobin,
            ProposerAlgorithm::WeightedRandom,
            ProposerAlgorithm::Fixed,
        ] {
            let enc = rlp::encode(&a);
            let dec: ProposerAlgorithm = rlp::decode(&enc).unwrap();
            assert_eq!(a, dec);
        }
        assert!(ProposerAlgorithm::from_tag(99).is_err());
    }

    #[test]
    fn schedule_lookup_returns_none_for_unscheduled() {
        let mut a = BTreeMap::new();
        a.insert(0, addr(1));
        let s = ProposerSchedule::new(0, 10, ProposerAlgorithm::Fixed, a).unwrap();
        assert_eq!(s.proposer_at(0), Some(addr(1)));
        assert_eq!(s.proposer_at(5), None);
    }
}
