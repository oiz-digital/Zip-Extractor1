//! Consensus types — validator set, BFT 2f+1 threshold, view-change.
//!
//! Type-and-codec layer. The driving state machine lives in `zbx-consensus`
//! and consumes these types verbatim.
//!
//! Discipline (matches sibling modules):
//! - `BTreeMap<Address, ValidatorInfo>` for canonical RLP. Address ordering
//!   uses the byte-lex order from `Ord` on `Address`.
//! - `validate()` runs in BOTH constructor AND `Decodable::decode`.
//! - Newtype `Encodable` delegations via `inner.rlp_append(s)` (LESSON #11).
//! - `item_count() != N` field-count gate at top of every `decode`.
//! - Voting power is `u64` (sum-overflow checked everywhere).

use std::collections::BTreeMap;

use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};
use serde::{Deserialize, Serialize};

use crate::address::Address;

// ---------------------------------------------------------------------------
// ValidatorInfo
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ValidatorInfo {
    pub address: Address,
    /// Voting power. MUST be > 0 for an active validator.
    pub voting_power: u64,
    /// Whether the validator is currently jailed (excluded from active set).
    pub jailed: bool,
    /// Block height at which `jailed` may flip back to false (0 if not jailed).
    pub jail_release_height: u64,
}

impl ValidatorInfo {
    pub fn new(
        address: Address,
        voting_power: u64,
        jailed: bool,
        jail_release_height: u64,
    ) -> Result<Self, DecoderError> {
        let v = Self {
            address,
            voting_power,
            jailed,
            jail_release_height,
        };
        v.validate()?;
        Ok(v)
    }

    pub fn validate(&self) -> Result<(), DecoderError> {
        if !self.jailed && self.voting_power == 0 {
            return Err(DecoderError::Custom(
                "active validator must have voting_power > 0",
            ));
        }
        if self.jailed && self.jail_release_height == 0 {
            return Err(DecoderError::Custom(
                "jailed validator must have jail_release_height > 0",
            ));
        }
        if !self.jailed && self.jail_release_height != 0 {
            return Err(DecoderError::Custom(
                "non-jailed validator must have jail_release_height == 0",
            ));
        }
        Ok(())
    }
}

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

impl Encodable for ValidatorInfo {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(4);
        append_address(s, &self.address);
        s.append(&self.voting_power);
        s.append(&self.jailed);
        s.append(&self.jail_release_height);
    }
}

impl Decodable for ValidatorInfo {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if rlp.item_count()? != 4 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        let v = Self {
            address: decode_address(&rlp.at(0)?)?,
            voting_power: rlp.val_at(1)?,
            jailed: rlp.val_at(2)?,
            jail_release_height: rlp.val_at(3)?,
        };
        v.validate()?;
        Ok(v)
    }
}

// ---------------------------------------------------------------------------
// ValidatorSet
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ValidatorSet {
    members: BTreeMap<Address, ValidatorInfo>,
}

impl ValidatorSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert(&mut self, info: ValidatorInfo) -> Result<(), DecoderError> {
        info.validate()?;
        self.members.insert(info.address, info);
        Ok(())
    }

    pub fn remove(&mut self, address: &Address) -> Option<ValidatorInfo> {
        self.members.remove(address)
    }

    pub fn get(&self, address: &Address) -> Option<&ValidatorInfo> {
        self.members.get(address)
    }

    pub fn len(&self) -> usize {
        self.members.len()
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    pub fn members(&self) -> &BTreeMap<Address, ValidatorInfo> {
        &self.members
    }

    /// Sum of voting power across NON-jailed validators (saturating).
    pub fn total_active_power(&self) -> u64 {
        self.members
            .values()
            .filter(|v| !v.jailed)
            .fold(0u64, |acc, v| acc.saturating_add(v.voting_power))
    }

    /// BFT 2f+1 quorum threshold: `⌊2n/3⌋ + 1`, capped at `n+1`.
    /// Any vote-power sum >= this value is sufficient to commit a block.
    pub fn quorum_threshold(&self) -> u64 {
        let n = self.total_active_power();
        let t = n.saturating_mul(2) / 3;
        t.saturating_add(1).min(n.saturating_add(1))
    }

    /// True iff `power` meets/exceeds the BFT 2f+1 quorum threshold.
    pub fn is_quorum(&self, power: u64) -> bool {
        power >= self.quorum_threshold()
    }
}

impl Encodable for ValidatorSet {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(self.members.len());
        for (addr, info) in &self.members {
            // Each member encoded as a 2-tuple [address, ValidatorInfo].
            s.begin_list(2);
            append_address(s, addr);
            info.rlp_append(s);
        }
    }
}

impl Decodable for ValidatorSet {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        let mut members = BTreeMap::new();
        let mut prev: Option<Address> = None;
        for item in rlp.iter() {
            if item.item_count()? != 2 {
                return Err(DecoderError::RlpIncorrectListLen);
            }
            let key = decode_address(&item.at(0)?)?;
            let info = ValidatorInfo::decode(&item.at(1)?)?;
            if info.address != key {
                return Err(DecoderError::Custom(
                    "ValidatorSet key/value address mismatch",
                ));
            }
            if let Some(ref p) = prev {
                if &key <= p {
                    return Err(DecoderError::Custom(
                        "ValidatorSet must be strictly ascending by address",
                    ));
                }
            }
            prev = Some(key);
            members.insert(key, info);
        }
        Ok(Self { members })
    }
}

// ---------------------------------------------------------------------------
// ConsensusParams
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConsensusParams {
    /// Maximum number of validators in the active set.
    pub max_validators: u32,
    /// Maximum voting power any single validator may hold.
    pub max_voting_power: u64,
    /// Target time between blocks (ms).
    pub block_time_ms: u64,
    /// Time after which a stuck round triggers view-change (ms).
    pub view_change_timeout_ms: u64,
    /// Number of blocks after which slashing evidence is no longer accepted.
    pub evidence_window_blocks: u64,
}

impl ConsensusParams {
    pub fn mainnet_default() -> Self {
        Self {
            max_validators: 128,
            max_voting_power: 1_000_000_000,
            block_time_ms: 5_000,
            view_change_timeout_ms: 10_000,
            evidence_window_blocks: 43_200, // ~2.5 days at 5s blocks (1 epoch)
        }
    }

    pub fn testnet_default() -> Self {
        Self {
            max_validators: 256,
            max_voting_power: 10_000_000_000,
            block_time_ms: 5_000,
            view_change_timeout_ms: 10_000,
            evidence_window_blocks: 17_280, // ~1 day at 5s blocks
        }
    }

    pub fn validate(&self) -> Result<(), DecoderError> {
        if self.max_validators == 0 {
            return Err(DecoderError::Custom("max_validators must be > 0"));
        }
        if self.max_voting_power == 0 {
            return Err(DecoderError::Custom("max_voting_power must be > 0"));
        }
        if self.block_time_ms == 0 {
            return Err(DecoderError::Custom("block_time_ms must be > 0"));
        }
        if self.view_change_timeout_ms == 0 {
            return Err(DecoderError::Custom(
                "view_change_timeout_ms must be > 0",
            ));
        }
        if self.view_change_timeout_ms < self.block_time_ms {
            return Err(DecoderError::Custom(
                "view_change_timeout_ms must be >= block_time_ms",
            ));
        }
        if self.evidence_window_blocks == 0 {
            return Err(DecoderError::Custom(
                "evidence_window_blocks must be > 0",
            ));
        }
        Ok(())
    }
}

impl Encodable for ConsensusParams {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(5);
        s.append(&self.max_validators);
        s.append(&self.max_voting_power);
        s.append(&self.block_time_ms);
        s.append(&self.view_change_timeout_ms);
        s.append(&self.evidence_window_blocks);
    }
}

impl Decodable for ConsensusParams {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if rlp.item_count()? != 5 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        let s = Self {
            max_validators: rlp.val_at(0)?,
            max_voting_power: rlp.val_at(1)?,
            block_time_ms: rlp.val_at(2)?,
            view_change_timeout_ms: rlp.val_at(3)?,
            evidence_window_blocks: rlp.val_at(4)?,
        };
        s.validate()?;
        Ok(s)
    }
}

// ---------------------------------------------------------------------------
// ViewChange — record of a leader-rotation event.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ViewChange {
    pub height: u64,
    /// Strictly-greater-than-zero view number; round 0 is implicit.
    pub view_number: u32,
    pub previous_leader: Address,
    pub new_leader: Address,
    /// Reason discriminant: 0 = timeout, 1 = bad-proposal, 2 = leader-jailed.
    pub reason: u8,
}

impl ViewChange {
    pub const REASON_TIMEOUT: u8 = 0;
    pub const REASON_BAD_PROPOSAL: u8 = 1;
    pub const REASON_LEADER_JAILED: u8 = 2;

    pub fn new(
        height: u64,
        view_number: u32,
        previous_leader: Address,
        new_leader: Address,
        reason: u8,
    ) -> Result<Self, DecoderError> {
        let v = Self {
            height,
            view_number,
            previous_leader,
            new_leader,
            reason,
        };
        v.validate()?;
        Ok(v)
    }

    pub fn validate(&self) -> Result<(), DecoderError> {
        if self.height == 0 {
            return Err(DecoderError::Custom("height must be > 0"));
        }
        if self.view_number == 0 {
            return Err(DecoderError::Custom("view_number must be > 0"));
        }
        if self.previous_leader == self.new_leader {
            return Err(DecoderError::Custom(
                "new_leader must differ from previous_leader",
            ));
        }
        if self.reason > Self::REASON_LEADER_JAILED {
            return Err(DecoderError::Custom("invalid view-change reason"));
        }
        Ok(())
    }
}

impl Encodable for ViewChange {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(5);
        s.append(&self.height);
        s.append(&self.view_number);
        append_address(s, &self.previous_leader);
        append_address(s, &self.new_leader);
        s.append(&self.reason);
    }
}

impl Decodable for ViewChange {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if rlp.item_count()? != 5 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        let v = Self {
            height: rlp.val_at(0)?,
            view_number: rlp.val_at(1)?,
            previous_leader: decode_address(&rlp.at(2)?)?,
            new_leader: decode_address(&rlp.at(3)?)?,
            reason: rlp.val_at(4)?,
        };
        v.validate()?;
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rlp::{decode, encode};

    fn addr(n: u8) -> Address {
        Address([n; 20])
    }

    // --- ValidatorInfo ---
    #[test]
    fn vinfo_active_requires_nonzero_power() {
        assert!(ValidatorInfo::new(addr(1), 0, false, 0).is_err());
    }

    #[test]
    fn vinfo_jailed_requires_release_height() {
        assert!(ValidatorInfo::new(addr(1), 100, true, 0).is_err());
    }

    #[test]
    fn vinfo_active_disallows_release_height() {
        assert!(ValidatorInfo::new(addr(1), 100, false, 50).is_err());
    }

    #[test]
    fn vinfo_rlp_round_trip_active() {
        let v = ValidatorInfo::new(addr(1), 100, false, 0).unwrap();
        let bytes = encode(&v);
        let back: ValidatorInfo = decode(&bytes).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn vinfo_rlp_round_trip_jailed() {
        let v = ValidatorInfo::new(addr(1), 100, true, 999).unwrap();
        let bytes = encode(&v);
        let back: ValidatorInfo = decode(&bytes).unwrap();
        assert_eq!(v, back);
    }

    // --- ValidatorSet quorum math ---
    #[test]
    fn quorum_threshold_n_equals_4() {
        // n=4 → ⌊8/3⌋ + 1 = 2 + 1 = 3
        let mut s = ValidatorSet::new();
        for i in 1..=4u8 {
            s.upsert(ValidatorInfo::new(addr(i), 1, false, 0).unwrap()).unwrap();
        }
        assert_eq!(s.total_active_power(), 4);
        assert_eq!(s.quorum_threshold(), 3);
        assert!(s.is_quorum(3));
        assert!(!s.is_quorum(2));
    }

    #[test]
    fn quorum_threshold_n_equals_7() {
        // n=7 → ⌊14/3⌋ + 1 = 4 + 1 = 5
        let mut s = ValidatorSet::new();
        for i in 1..=7u8 {
            s.upsert(ValidatorInfo::new(addr(i), 1, false, 0).unwrap()).unwrap();
        }
        assert_eq!(s.quorum_threshold(), 5);
    }

    #[test]
    fn quorum_threshold_n_equals_zero() {
        let s = ValidatorSet::new();
        assert_eq!(s.total_active_power(), 0);
        assert_eq!(s.quorum_threshold(), 1);
        assert!(!s.is_quorum(0));
        assert!(s.is_quorum(1));
    }

    #[test]
    fn quorum_excludes_jailed_power() {
        let mut s = ValidatorSet::new();
        s.upsert(ValidatorInfo::new(addr(1), 100, false, 0).unwrap()).unwrap();
        s.upsert(ValidatorInfo::new(addr(2), 1000, true, 999).unwrap()).unwrap();
        // total_active = 100, threshold = ⌊200/3⌋+1 = 67
        assert_eq!(s.total_active_power(), 100);
        assert_eq!(s.quorum_threshold(), 67);
    }

    // --- ValidatorSet RLP ---
    #[test]
    fn vset_rlp_round_trip() {
        let mut s = ValidatorSet::new();
        s.upsert(ValidatorInfo::new(addr(3), 50, false, 0).unwrap()).unwrap();
        s.upsert(ValidatorInfo::new(addr(1), 100, false, 0).unwrap()).unwrap();
        s.upsert(ValidatorInfo::new(addr(2), 200, true, 5000).unwrap()).unwrap();
        let bytes = encode(&s);
        let back: ValidatorSet = decode(&bytes).unwrap();
        assert_eq!(s, back);
        // Canonical order: addr(1), addr(2), addr(3)
        let addrs: Vec<&Address> = back.members().keys().collect();
        assert_eq!(addrs, vec![&addr(1), &addr(2), &addr(3)]);
    }

    #[test]
    fn vset_decode_rejects_unsorted() {
        let mut s = RlpStream::new_list(2);
        s.begin_list(2);
        append_address(&mut s, &addr(5));
        ValidatorInfo::new(addr(5), 1, false, 0)
            .unwrap()
            .rlp_append(&mut s);
        s.begin_list(2);
        append_address(&mut s, &addr(2)); // out of order
        ValidatorInfo::new(addr(2), 1, false, 0)
            .unwrap()
            .rlp_append(&mut s);
        let bytes = s.out();
        let r: Result<ValidatorSet, _> = decode(&bytes);
        assert!(matches!(r, Err(DecoderError::Custom(_))));
    }

    #[test]
    fn vset_decode_rejects_key_value_mismatch() {
        let mut s = RlpStream::new_list(1);
        s.begin_list(2);
        append_address(&mut s, &addr(1));
        ValidatorInfo::new(addr(2), 1, false, 0) // mismatched address inside info
            .unwrap()
            .rlp_append(&mut s);
        let bytes = s.out();
        let r: Result<ValidatorSet, _> = decode(&bytes);
        assert!(matches!(r, Err(DecoderError::Custom(_))));
    }

    // --- ConsensusParams ---
    #[test]
    fn params_mainnet_default_validates() {
        ConsensusParams::mainnet_default().validate().unwrap();
    }

    #[test]
    fn params_testnet_default_validates() {
        ConsensusParams::testnet_default().validate().unwrap();
    }

    #[test]
    fn params_rejects_view_timeout_below_block_time() {
        let mut p = ConsensusParams::mainnet_default();
        p.view_change_timeout_ms = p.block_time_ms - 1;
        assert!(p.validate().is_err());
    }

    #[test]
    fn params_rlp_round_trip() {
        let p = ConsensusParams::mainnet_default();
        let bytes = encode(&p);
        let back: ConsensusParams = decode(&bytes).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn params_decode_rejects_wrong_field_count() {
        let mut s = RlpStream::new_list(4);
        s.append(&128u32);
        s.append(&1u64);
        s.append(&2_000u64);
        s.append(&8_000u64);
        let bytes = s.out();
        let r: Result<ConsensusParams, _> = decode(&bytes);
        assert!(matches!(r, Err(DecoderError::RlpIncorrectListLen)));
    }

    // --- ViewChange ---
    #[test]
    fn vc_rejects_same_leader() {
        assert!(ViewChange::new(
            10,
            1,
            addr(1),
            addr(1),
            ViewChange::REASON_TIMEOUT
        )
        .is_err());
    }

    #[test]
    fn vc_rejects_zero_view_number() {
        assert!(ViewChange::new(
            10,
            0,
            addr(1),
            addr(2),
            ViewChange::REASON_TIMEOUT
        )
        .is_err());
    }

    #[test]
    fn vc_rejects_invalid_reason() {
        assert!(ViewChange::new(10, 1, addr(1), addr(2), 99).is_err());
    }

    #[test]
    fn vc_rlp_round_trip() {
        let vc = ViewChange::new(
            10,
            3,
            addr(1),
            addr(2),
            ViewChange::REASON_BAD_PROPOSAL,
        )
        .unwrap();
        let bytes = encode(&vc);
        let back: ViewChange = decode(&bytes).unwrap();
        assert_eq!(vc, back);
    }
}
