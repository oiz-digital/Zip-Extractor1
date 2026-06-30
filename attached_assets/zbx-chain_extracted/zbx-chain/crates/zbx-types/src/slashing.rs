//! Slashing — evidence, faults, penalties, registry.
//!
//! Type-and-codec layer. Detection (catching equivocation in the BFT loop),
//! reporting (evidence-tx admission), and execution (stake burn / jail) live
//! in `zbx-consensus`/`zbx-staking` and consume these types.
//!
//! Discipline (matches `execution.rs`, `governance.rs`, `validation.rs`):
//! - `validate()` runs in BOTH constructor AND `Decodable::decode`.
//! - `BTreeMap<H256, SlashingRecord>` keyed by evidence hash → canonical RLP,
//!   sorted-strict-monotone re-checked on decode.
//! - Newtype `Encodable` delegations via `inner.rlp_append(s)` (LESSON #11).
//! - Field-count gate at top of every `decode`.
//! - Penalty fractions stored as `(numer, denom)` with `denom > 0` and
//!   `numer <= denom` invariants.

use std::collections::BTreeMap;

use primitive_types::H256;
use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};
use serde::{Deserialize, Serialize};

use crate::address::Address;

// ---------------------------------------------------------------------------
// SlashingFault — taxonomy of misbehavior.
// ---------------------------------------------------------------------------

/// Discriminant for slashable validator misbehavior.
///
/// Encoded as a single byte. Append-only: never reorder or remove variants.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum SlashingFault {
    /// Two distinct blocks signed at the same height/view by the same validator.
    DoubleSign = 0,
    /// Two distinct PRE-VOTEs (or two distinct PRE-COMMITs) at the same
    /// height/round by the same validator.
    DoubleVote = 1,
    /// Generic equivocation — conflicting consensus messages that don't fit
    /// the strict DoubleSign/DoubleVote shapes (e.g. cross-round reuse).
    Equivocation = 2,
    /// Sustained absence — failed liveness threshold over the configured window.
    Inactivity = 3,
    /// Long-range fork: signing a header below the validator-set finality cutoff.
    LongRangeAttack = 4,
}

impl SlashingFault {
    pub fn to_u8(self) -> u8 {
        self as u8
    }
    pub fn from_u8(b: u8) -> Result<Self, DecoderError> {
        match b {
            0 => Ok(Self::DoubleSign),
            1 => Ok(Self::DoubleVote),
            2 => Ok(Self::Equivocation),
            3 => Ok(Self::Inactivity),
            4 => Ok(Self::LongRangeAttack),
            _ => Err(DecoderError::Custom("invalid SlashingFault byte")),
        }
    }
}

impl Encodable for SlashingFault {
    fn rlp_append(&self, s: &mut RlpStream) {
        self.to_u8().rlp_append(s);
    }
}

impl Decodable for SlashingFault {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        let b: u8 = rlp.as_val()?;
        Self::from_u8(b)
    }
}

// ---------------------------------------------------------------------------
// SlashingPenalty — what the chain does once evidence is confirmed.
// ---------------------------------------------------------------------------

/// Action taken against a misbehaving validator. Encoded as a tagged list.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SlashingPenalty {
    /// Burn `numer/denom` of the validator's stake.
    BurnFraction { numer: u32, denom: u32 },
    /// Jail (no rewards, removed from active set) until `until_height`.
    Jail { until_height: u64 },
    /// Permanent removal from the validator set.
    Kick,
}

impl SlashingPenalty {
    pub fn validate(&self) -> Result<(), DecoderError> {
        match self {
            Self::BurnFraction { numer, denom } => {
                if *denom == 0 {
                    return Err(DecoderError::Custom("denom must be > 0"));
                }
                if numer > denom {
                    return Err(DecoderError::Custom("numer must be <= denom"));
                }
                Ok(())
            }
            Self::Jail { until_height } => {
                if *until_height == 0 {
                    return Err(DecoderError::Custom("until_height must be > 0"));
                }
                Ok(())
            }
            Self::Kick => Ok(()),
        }
    }

    fn tag(&self) -> u8 {
        match self {
            Self::BurnFraction { .. } => 0,
            Self::Jail { .. } => 1,
            Self::Kick => 2,
        }
    }
}

impl Encodable for SlashingPenalty {
    fn rlp_append(&self, s: &mut RlpStream) {
        match self {
            Self::BurnFraction { numer, denom } => {
                s.begin_list(3);
                s.append(&self.tag());
                s.append(numer);
                s.append(denom);
            }
            Self::Jail { until_height } => {
                s.begin_list(2);
                s.append(&self.tag());
                s.append(until_height);
            }
            Self::Kick => {
                s.begin_list(1);
                s.append(&self.tag());
            }
        }
    }
}

impl Decodable for SlashingPenalty {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        let n = rlp.item_count()?;
        if n == 0 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        let tag: u8 = rlp.val_at(0)?;
        let p = match tag {
            0 => {
                if n != 3 {
                    return Err(DecoderError::RlpIncorrectListLen);
                }
                Self::BurnFraction {
                    numer: rlp.val_at(1)?,
                    denom: rlp.val_at(2)?,
                }
            }
            1 => {
                if n != 2 {
                    return Err(DecoderError::RlpIncorrectListLen);
                }
                Self::Jail {
                    until_height: rlp.val_at(1)?,
                }
            }
            2 => {
                if n != 1 {
                    return Err(DecoderError::RlpIncorrectListLen);
                }
                Self::Kick
            }
            _ => return Err(DecoderError::Custom("invalid SlashingPenalty tag")),
        };
        p.validate()?;
        Ok(p)
    }
}

// ---------------------------------------------------------------------------
// SlashingEvidence — the cryptographic claim that a validator misbehaved.
// ---------------------------------------------------------------------------

/// Two conflicting messages signed by the same validator. Stored as their
/// hashes; raw payloads live in the underlying tx so block-state stays small.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SlashingEvidence {
    pub fault: SlashingFault,
    pub validator: Address,
    /// Height at which the fault occurred.
    pub fault_height: u64,
    /// View / round number within `fault_height` (0 if not applicable).
    pub fault_view: u32,
    /// Hash of the first conflicting message.
    pub evidence_a_hash: H256,
    /// Hash of the second conflicting message. MUST differ from `evidence_a_hash`.
    pub evidence_b_hash: H256,
    /// Block height at which the evidence was reported on chain.
    pub reported_at_height: u64,
}

impl SlashingEvidence {
    pub fn new(
        fault: SlashingFault,
        validator: Address,
        fault_height: u64,
        fault_view: u32,
        evidence_a_hash: H256,
        evidence_b_hash: H256,
        reported_at_height: u64,
    ) -> Result<Self, DecoderError> {
        let s = Self {
            fault,
            validator,
            fault_height,
            fault_view,
            evidence_a_hash,
            evidence_b_hash,
            reported_at_height,
        };
        s.validate()?;
        Ok(s)
    }

    pub fn validate(&self) -> Result<(), DecoderError> {
        if self.fault_height == 0 {
            return Err(DecoderError::Custom("fault_height must be > 0"));
        }
        if self.reported_at_height < self.fault_height {
            return Err(DecoderError::Custom(
                "reported_at_height must be >= fault_height",
            ));
        }
        // Inactivity is the only fault where the two evidence hashes may
        // legitimately match (both reference the same liveness window).
        if matches!(
            self.fault,
            SlashingFault::DoubleSign
                | SlashingFault::DoubleVote
                | SlashingFault::Equivocation
                | SlashingFault::LongRangeAttack
        ) && self.evidence_a_hash == self.evidence_b_hash
        {
            return Err(DecoderError::Custom(
                "evidence_a_hash and evidence_b_hash must differ for non-Inactivity faults",
            ));
        }
        Ok(())
    }
}

// Local 20-byte address codec helper (crate-private; LESSON #12).
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

// Local 32-byte H256 codec helper.
fn append_h256(s: &mut RlpStream, h: &H256) {
    s.append(&h.as_bytes());
}
fn decode_h256(rlp: &Rlp) -> Result<H256, DecoderError> {
    let bytes: Vec<u8> = rlp.as_val()?;
    if bytes.len() != 32 {
        return Err(DecoderError::Custom("H256 must be 32 bytes"));
    }
    Ok(H256::from_slice(&bytes))
}

impl Encodable for SlashingEvidence {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(7);
        s.append(&self.fault);
        append_address(s, &self.validator);
        s.append(&self.fault_height);
        s.append(&self.fault_view);
        append_h256(s, &self.evidence_a_hash);
        append_h256(s, &self.evidence_b_hash);
        s.append(&self.reported_at_height);
    }
}

impl Decodable for SlashingEvidence {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if rlp.item_count()? != 7 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        let s = Self {
            fault: SlashingFault::decode(&rlp.at(0)?)?,
            validator: decode_address(&rlp.at(1)?)?,
            fault_height: rlp.val_at(2)?,
            fault_view: rlp.val_at(3)?,
            evidence_a_hash: decode_h256(&rlp.at(4)?)?,
            evidence_b_hash: decode_h256(&rlp.at(5)?)?,
            reported_at_height: rlp.val_at(6)?,
        };
        s.validate()?;
        Ok(s)
    }
}

// ---------------------------------------------------------------------------
// SlashingParams — per-fault penalty mapping.
// ---------------------------------------------------------------------------

/// Maps each fault kind to its default penalty. Stored canonically (sorted
/// by `SlashingFault::to_u8`) so the RLP encoding is deterministic.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SlashingParams {
    map: BTreeMap<u8, SlashingPenalty>,
}

impl SlashingParams {
    pub fn new() -> Self {
        Self {
            map: BTreeMap::new(),
        }
    }

    /// Mainnet defaults, conservative.
    pub fn mainnet_default() -> Self {
        let mut s = Self::new();
        s.set(
            SlashingFault::DoubleSign,
            SlashingPenalty::BurnFraction { numer: 1, denom: 20 }, // 5%
        )
        .unwrap();
        s.set(
            SlashingFault::DoubleVote,
            SlashingPenalty::BurnFraction { numer: 1, denom: 50 }, // 2%
        )
        .unwrap();
        s.set(
            SlashingFault::Equivocation,
            SlashingPenalty::BurnFraction { numer: 1, denom: 100 }, // 1%
        )
        .unwrap();
        s.set(SlashingFault::Inactivity, SlashingPenalty::Jail { until_height: 1 })
            .unwrap();
        s.set(SlashingFault::LongRangeAttack, SlashingPenalty::Kick)
            .unwrap();
        s
    }

    pub fn set(
        &mut self,
        fault: SlashingFault,
        penalty: SlashingPenalty,
    ) -> Result<(), DecoderError> {
        penalty.validate()?;
        self.map.insert(fault.to_u8(), penalty);
        Ok(())
    }

    pub fn get(&self, fault: SlashingFault) -> Option<&SlashingPenalty> {
        self.map.get(&fault.to_u8())
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

impl Default for SlashingParams {
    fn default() -> Self {
        Self::new()
    }
}

impl Encodable for SlashingParams {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(self.map.len());
        for (tag, pen) in &self.map {
            s.begin_list(2);
            s.append(tag);
            s.append(pen);
        }
    }
}

impl Decodable for SlashingParams {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        let mut map = BTreeMap::new();
        let mut prev: Option<u8> = None;
        for item in rlp.iter() {
            if item.item_count()? != 2 {
                return Err(DecoderError::RlpIncorrectListLen);
            }
            let tag: u8 = item.val_at(0)?;
            // Validate tag falls within the known SlashingFault range.
            SlashingFault::from_u8(tag)?;
            if let Some(p) = prev {
                if tag <= p {
                    return Err(DecoderError::Custom(
                        "SlashingParams entries must be strictly ascending by fault tag",
                    ));
                }
            }
            prev = Some(tag);
            let pen = SlashingPenalty::decode(&item.at(1)?)?;
            map.insert(tag, pen);
        }
        Ok(Self { map })
    }
}

// ---------------------------------------------------------------------------
// SlashingRecord + SlashingRegistry — applied evidence + lookup.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SlashingRecord {
    pub evidence: SlashingEvidence,
    pub penalty: SlashingPenalty,
    pub applied_at_height: u64,
}

impl SlashingRecord {
    pub fn new(
        evidence: SlashingEvidence,
        penalty: SlashingPenalty,
        applied_at_height: u64,
    ) -> Result<Self, DecoderError> {
        let s = Self {
            evidence,
            penalty,
            applied_at_height,
        };
        s.validate()?;
        Ok(s)
    }

    pub fn validate(&self) -> Result<(), DecoderError> {
        self.evidence.validate()?;
        self.penalty.validate()?;
        if self.applied_at_height < self.evidence.reported_at_height {
            return Err(DecoderError::Custom(
                "applied_at_height must be >= reported_at_height",
            ));
        }
        Ok(())
    }
}

impl Encodable for SlashingRecord {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(3);
        s.append(&self.evidence);
        s.append(&self.penalty);
        s.append(&self.applied_at_height);
    }
}

impl Decodable for SlashingRecord {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if rlp.item_count()? != 3 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        let s = Self {
            evidence: SlashingEvidence::decode(&rlp.at(0)?)?,
            penalty: SlashingPenalty::decode(&rlp.at(1)?)?,
            applied_at_height: rlp.val_at(2)?,
        };
        s.validate()?;
        Ok(s)
    }
}

/// Indexed by evidence hash (consensus computes this from the conflicting
/// message tuple). Sorted-strict-monotone on decode.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SlashingRegistry {
    records: BTreeMap<H256, SlashingRecord>,
}

impl SlashingRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(
        &mut self,
        evidence_hash: H256,
        record: SlashingRecord,
    ) -> Result<(), DecoderError> {
        if self.records.contains_key(&evidence_hash) {
            return Err(DecoderError::Custom(
                "duplicate evidence_hash in SlashingRegistry",
            ));
        }
        record.validate()?;
        self.records.insert(evidence_hash, record);
        Ok(())
    }

    pub fn get(&self, evidence_hash: &H256) -> Option<&SlashingRecord> {
        self.records.get(evidence_hash)
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn records(&self) -> &BTreeMap<H256, SlashingRecord> {
        &self.records
    }
}

impl Encodable for SlashingRegistry {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(self.records.len());
        for (h, rec) in &self.records {
            s.begin_list(2);
            append_h256(s, h);
            s.append(rec);
        }
    }
}

impl Decodable for SlashingRegistry {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        let mut records = BTreeMap::new();
        let mut prev: Option<H256> = None;
        for item in rlp.iter() {
            if item.item_count()? != 2 {
                return Err(DecoderError::RlpIncorrectListLen);
            }
            let h = decode_h256(&item.at(0)?)?;
            if let Some(p) = prev {
                if h <= p {
                    return Err(DecoderError::Custom(
                        "SlashingRegistry must be strictly ascending by evidence_hash",
                    ));
                }
            }
            prev = Some(h);
            let rec = SlashingRecord::decode(&item.at(1)?)?;
            records.insert(h, rec);
        }
        Ok(Self { records })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rlp::{decode, encode};

    fn addr(n: u8) -> Address {
        Address([n; 20])
    }

    fn h(n: u8) -> H256 {
        let mut b = [0u8; 32];
        b[31] = n;
        H256::from(b)
    }

    fn ev(fault: SlashingFault, ah: u8, bh: u8) -> SlashingEvidence {
        SlashingEvidence::new(fault, addr(1), 100, 0, h(ah), h(bh), 105).unwrap()
    }

    // --- SlashingFault ---
    #[test]
    fn fault_roundtrip_all_variants() {
        for f in [
            SlashingFault::DoubleSign,
            SlashingFault::DoubleVote,
            SlashingFault::Equivocation,
            SlashingFault::Inactivity,
            SlashingFault::LongRangeAttack,
        ] {
            let bytes = encode(&f);
            let back: SlashingFault = decode(&bytes).unwrap();
            assert_eq!(f, back);
        }
    }

    #[test]
    fn fault_decode_rejects_invalid_byte() {
        let bytes = encode(&99u8);
        let r: Result<SlashingFault, _> = decode(&bytes);
        assert!(r.is_err());
    }

    // --- SlashingPenalty ---
    #[test]
    fn penalty_burn_validates_denom_nonzero() {
        let p = SlashingPenalty::BurnFraction { numer: 1, denom: 0 };
        assert!(p.validate().is_err());
    }

    #[test]
    fn penalty_burn_validates_numer_le_denom() {
        let p = SlashingPenalty::BurnFraction { numer: 5, denom: 4 };
        assert!(p.validate().is_err());
    }

    #[test]
    fn penalty_jail_validates_height_nonzero() {
        let p = SlashingPenalty::Jail { until_height: 0 };
        assert!(p.validate().is_err());
    }

    #[test]
    fn penalty_burn_roundtrip() {
        let p = SlashingPenalty::BurnFraction { numer: 1, denom: 20 };
        let bytes = encode(&p);
        let back: SlashingPenalty = decode(&bytes).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn penalty_jail_roundtrip() {
        let p = SlashingPenalty::Jail { until_height: 42 };
        let bytes = encode(&p);
        let back: SlashingPenalty = decode(&bytes).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn penalty_kick_roundtrip() {
        let p = SlashingPenalty::Kick;
        let bytes = encode(&p);
        let back: SlashingPenalty = decode(&bytes).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn penalty_decode_rejects_unknown_tag() {
        let mut s = RlpStream::new_list(1);
        s.append(&99u8);
        let bytes = s.out();
        let r: Result<SlashingPenalty, _> = decode(&bytes);
        assert!(r.is_err());
    }

    #[test]
    fn penalty_burn_decode_rejects_wrong_arity() {
        let mut s = RlpStream::new_list(2); // burn needs 3
        s.append(&0u8);
        s.append(&1u32);
        let bytes = s.out();
        let r: Result<SlashingPenalty, _> = decode(&bytes);
        assert!(matches!(r, Err(DecoderError::RlpIncorrectListLen)));
    }

    // --- SlashingEvidence ---
    #[test]
    fn evidence_validates_fault_height_nonzero() {
        let r = SlashingEvidence::new(
            SlashingFault::DoubleSign,
            addr(1),
            0,
            0,
            h(1),
            h(2),
            10,
        );
        assert!(r.is_err());
    }

    #[test]
    fn evidence_validates_reported_ge_fault_height() {
        let r = SlashingEvidence::new(
            SlashingFault::DoubleSign,
            addr(1),
            10,
            0,
            h(1),
            h(2),
            5,
        );
        assert!(r.is_err());
    }

    #[test]
    fn evidence_double_sign_rejects_equal_hashes() {
        let r = SlashingEvidence::new(
            SlashingFault::DoubleSign,
            addr(1),
            10,
            0,
            h(1),
            h(1),
            10,
        );
        assert!(r.is_err());
    }

    #[test]
    fn evidence_inactivity_allows_equal_hashes() {
        let r = SlashingEvidence::new(
            SlashingFault::Inactivity,
            addr(1),
            10,
            0,
            h(1),
            h(1),
            20,
        );
        assert!(r.is_ok());
    }

    #[test]
    fn evidence_rlp_round_trip() {
        let e = ev(SlashingFault::DoubleVote, 7, 8);
        let bytes = encode(&e);
        let back: SlashingEvidence = decode(&bytes).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn evidence_decode_rejects_wrong_field_count() {
        let mut s = RlpStream::new_list(6);
        s.append(&SlashingFault::DoubleSign);
        append_address(&mut s, &addr(1));
        s.append(&100u64);
        s.append(&0u32);
        append_h256(&mut s, &h(1));
        append_h256(&mut s, &h(2));
        let bytes = s.out();
        let r: Result<SlashingEvidence, _> = decode(&bytes);
        assert!(matches!(r, Err(DecoderError::RlpIncorrectListLen)));
    }

    // --- SlashingParams ---
    #[test]
    fn params_mainnet_default_has_all_faults() {
        let p = SlashingParams::mainnet_default();
        assert_eq!(p.len(), 5);
        assert!(p.get(SlashingFault::DoubleSign).is_some());
        assert!(p.get(SlashingFault::LongRangeAttack).is_some());
    }

    #[test]
    fn params_set_validates_penalty() {
        let mut p = SlashingParams::new();
        let r = p.set(
            SlashingFault::DoubleSign,
            SlashingPenalty::BurnFraction { numer: 5, denom: 4 },
        );
        assert!(r.is_err());
    }

    #[test]
    fn params_rlp_round_trip() {
        let p = SlashingParams::mainnet_default();
        let bytes = encode(&p);
        let back: SlashingParams = decode(&bytes).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn params_decode_rejects_unsorted() {
        let mut s = RlpStream::new_list(2);
        s.begin_list(2);
        s.append(&3u8); // Inactivity
        s.append(&SlashingPenalty::Kick);
        s.begin_list(2);
        s.append(&1u8); // DoubleVote — out of order
        s.append(&SlashingPenalty::Kick);
        let bytes = s.out();
        let r: Result<SlashingParams, _> = decode(&bytes);
        assert!(matches!(r, Err(DecoderError::Custom(_))));
    }

    #[test]
    fn params_decode_rejects_unknown_tag() {
        let mut s = RlpStream::new_list(1);
        s.begin_list(2);
        s.append(&99u8);
        s.append(&SlashingPenalty::Kick);
        let bytes = s.out();
        let r: Result<SlashingParams, _> = decode(&bytes);
        assert!(r.is_err());
    }

    // --- SlashingRecord ---
    #[test]
    fn record_validates_applied_ge_reported() {
        let e = ev(SlashingFault::DoubleSign, 1, 2);
        let r = SlashingRecord::new(
            e,
            SlashingPenalty::BurnFraction { numer: 1, denom: 20 },
            // reported_at_height in ev() is 105, applied = 100 → invalid
            100,
        );
        assert!(r.is_err());
    }

    #[test]
    fn record_rlp_round_trip() {
        let e = ev(SlashingFault::DoubleSign, 1, 2);
        let r = SlashingRecord::new(
            e,
            SlashingPenalty::BurnFraction { numer: 1, denom: 20 },
            110,
        )
        .unwrap();
        let bytes = encode(&r);
        let back: SlashingRecord = decode(&bytes).unwrap();
        assert_eq!(r, back);
    }

    // --- SlashingRegistry ---
    #[test]
    fn registry_insert_then_get() {
        let mut reg = SlashingRegistry::new();
        let e = ev(SlashingFault::DoubleSign, 1, 2);
        let rec =
            SlashingRecord::new(e, SlashingPenalty::BurnFraction { numer: 1, denom: 20 }, 110)
                .unwrap();
        reg.insert(h(42), rec.clone()).unwrap();
        assert_eq!(reg.len(), 1);
        assert_eq!(reg.get(&h(42)), Some(&rec));
    }

    #[test]
    fn registry_rejects_duplicate_hash() {
        let mut reg = SlashingRegistry::new();
        let e = ev(SlashingFault::DoubleSign, 1, 2);
        let rec =
            SlashingRecord::new(e, SlashingPenalty::BurnFraction { numer: 1, denom: 20 }, 110)
                .unwrap();
        reg.insert(h(42), rec.clone()).unwrap();
        assert!(reg.insert(h(42), rec).is_err());
    }

    #[test]
    fn registry_rlp_round_trip_preserves_canonical_order() {
        let mut reg = SlashingRegistry::new();
        let rec_for = |a: u8, b: u8| {
            SlashingRecord::new(
                ev(SlashingFault::DoubleVote, a, b),
                SlashingPenalty::BurnFraction { numer: 1, denom: 50 },
                110,
            )
            .unwrap()
        };
        reg.insert(h(9), rec_for(1, 2)).unwrap();
        reg.insert(h(2), rec_for(3, 4)).unwrap();
        reg.insert(h(7), rec_for(5, 6)).unwrap();
        let bytes = encode(&reg);
        let back: SlashingRegistry = decode(&bytes).unwrap();
        assert_eq!(reg, back);
        // Verify keys come out in canonical (BTreeMap-sorted) order.
        let keys: Vec<&H256> = back.records().keys().collect();
        assert!(keys.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn registry_decode_rejects_unsorted() {
        let mut s = RlpStream::new_list(2);
        let r1 = SlashingRecord::new(
            ev(SlashingFault::DoubleSign, 1, 2),
            SlashingPenalty::Kick,
            110,
        )
        .unwrap();
        let r2 = SlashingRecord::new(
            ev(SlashingFault::DoubleSign, 3, 4),
            SlashingPenalty::Kick,
            110,
        )
        .unwrap();
        s.begin_list(2);
        append_h256(&mut s, &h(9));
        s.append(&r1);
        s.begin_list(2);
        append_h256(&mut s, &h(2)); // out of order
        s.append(&r2);
        let bytes = s.out();
        let r: Result<SlashingRegistry, _> = decode(&bytes);
        assert!(matches!(r, Err(DecoderError::Custom(_))));
    }
}
