//! Block finality — checkpoint vote tracking and 2/3+1 threshold logic.
//!
//! Closes Item #2 (consensus security) sub-bullet "block finality logic".
//! BFT-style finality: a block at height H is **finalized** once validators
//! controlling > 2/3 of total active voting power have signed a finality
//! vote referencing `(H, block_hash)`. The pre-finalized state is
//! **justified** (the block has any vote and is therefore a valid head
//! candidate, but cannot be reverted only by the simple-majority rule).
//!
//! Discipline:
//! - `BTreeMap`/`BTreeSet` for canonical RLP. `validate()` runs in BOTH
//!   constructor AND `Decodable::decode`.
//! - `s.append(&inner)` inside `begin_list(N)` — never the naked
//!   `inner.rlp_append(s)`.
//! - Newtype `Encodable` impls use direct delegation only when the inner
//!   type already supports `s.append(&_)` (LESSON #11).

use std::collections::BTreeMap;

use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};
use serde::{Deserialize, Serialize};

use crate::address::Address;
use crate::consensus::ValidatorSet;
use crate::H256;

// ---------------------------------------------------------------------------
// FinalityStatus — coarse 3-state lifecycle of a checkpoint.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum FinalityStatus {
    /// No vote yet recorded for this `(height, block_hash)`.
    Pending,
    /// At least one valid vote, but below the 2/3+1 threshold.
    Justified,
    /// Voting power above 2/3+1 — block is irreversible by the protocol.
    Finalized,
}

impl FinalityStatus {
    pub fn tag(self) -> u8 {
        match self {
            Self::Pending   => 0,
            Self::Justified => 1,
            Self::Finalized => 2,
        }
    }

    pub fn from_tag(t: u8) -> Result<Self, DecoderError> {
        match t {
            0 => Ok(Self::Pending),
            1 => Ok(Self::Justified),
            2 => Ok(Self::Finalized),
            _ => Err(DecoderError::Custom("invalid FinalityStatus tag")),
        }
    }
}

impl Encodable for FinalityStatus {
    fn rlp_append(&self, s: &mut RlpStream) { self.tag().rlp_append(s); }
}

impl Decodable for FinalityStatus {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        Self::from_tag(rlp.as_val()?)
    }
}

// ---------------------------------------------------------------------------
// Local Address codec helpers.
// ---------------------------------------------------------------------------

fn append_address(s: &mut RlpStream, a: &Address) { s.append(&a.0.as_ref()); }

fn decode_address(rlp: &Rlp) -> Result<Address, DecoderError> {
    let bytes: Vec<u8> = rlp.as_val()?;
    if bytes.len() != 20 {
        return Err(DecoderError::Custom("address must be 20 bytes"));
    }
    let mut out = [0u8; 20];
    out.copy_from_slice(&bytes);
    Ok(Address(out))
}

fn append_h256(s: &mut RlpStream, h: &H256) { s.append(&h.as_bytes()); }

fn decode_h256(rlp: &Rlp) -> Result<H256, DecoderError> {
    let bytes: Vec<u8> = rlp.as_val()?;
    if bytes.len() != 32 {
        return Err(DecoderError::Custom("H256 must be 32 bytes"));
    }
    Ok(H256::from_slice(&bytes))
}

// ---------------------------------------------------------------------------
// FinalityVote — single validator's signed pre-commit on `(height, hash)`.
// The signature itself is opaque bytes here; verification happens in
// zbx-consensus against the validator's BLS pubkey.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FinalityVote {
    pub voter:      Address,
    pub height:     u64,
    pub block_hash: H256,
    pub signature:  Vec<u8>,    // BLS or EdDSA, ≤ 96 bytes
}

impl FinalityVote {
    pub const MAX_SIG_LEN: usize = 96;

    pub fn new(
        voter: Address,
        height: u64,
        block_hash: H256,
        signature: Vec<u8>,
    ) -> Result<Self, DecoderError> {
        let v = Self { voter, height, block_hash, signature };
        v.validate()?;
        Ok(v)
    }

    pub fn validate(&self) -> Result<(), DecoderError> {
        if self.signature.is_empty() {
            return Err(DecoderError::Custom("FinalityVote: empty signature"));
        }
        if self.signature.len() > Self::MAX_SIG_LEN {
            return Err(DecoderError::Custom("FinalityVote: signature too long"));
        }
        Ok(())
    }
}

impl Encodable for FinalityVote {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(4);
        append_address(s, &self.voter);
        s.append(&self.height);
        append_h256(s, &self.block_hash);
        s.append(&self.signature);
    }
}

impl Decodable for FinalityVote {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if rlp.item_count()? != 4 { return Err(DecoderError::RlpIncorrectListLen); }
        let v = Self {
            voter:      decode_address(&rlp.at(0)?)?,
            height:     rlp.val_at(1)?,
            block_hash: decode_h256(&rlp.at(2)?)?,
            signature:  rlp.val_at(3)?,
        };
        v.validate()?;
        Ok(v)
    }
}

// ---------------------------------------------------------------------------
// FinalityCheckpoint — accumulated votes for one `(height, block_hash)`.
// Duplicate votes from the same `voter` are silently overwritten on
// `add_vote` (last-write-wins) — this matches the BFT property that a
// validator who flips their vote is detectable separately as equivocation
// (handled by `slashing.rs::SlashingFault::DuplicateVote`).
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FinalityCheckpoint {
    pub height:     u64,
    pub block_hash: H256,
    pub votes:      BTreeMap<Address, FinalityVote>,
}

impl FinalityCheckpoint {
    pub fn new(height: u64, block_hash: H256) -> Self {
        Self { height, block_hash, votes: BTreeMap::new() }
    }

    pub fn validate(&self) -> Result<(), DecoderError> {
        for (voter, v) in &self.votes {
            if &v.voter != voter {
                return Err(DecoderError::Custom("FinalityCheckpoint: vote/voter mismatch"));
            }
            if v.height != self.height {
                return Err(DecoderError::Custom("FinalityCheckpoint: vote height mismatch"));
            }
            if v.block_hash != self.block_hash {
                return Err(DecoderError::Custom("FinalityCheckpoint: vote hash mismatch"));
            }
            v.validate()?;
        }
        Ok(())
    }

    /// Add a vote. Returns `Err` if the vote does not match this
    /// checkpoint's `(height, block_hash)`.
    pub fn add_vote(&mut self, vote: FinalityVote) -> Result<(), DecoderError> {
        if vote.height != self.height {
            return Err(DecoderError::Custom("vote.height ≠ checkpoint.height"));
        }
        if vote.block_hash != self.block_hash {
            return Err(DecoderError::Custom("vote.block_hash ≠ checkpoint.block_hash"));
        }
        vote.validate()?;
        self.votes.insert(vote.voter, vote);
        Ok(())
    }

    /// Sum the voting power of the validators that have voted, restricted
    /// to non-jailed members of `set`. Votes from non-members or jailed
    /// validators are silently ignored — they have zero weight.
    pub fn voted_power(&self, set: &ValidatorSet) -> u64 {
        let mut sum: u64 = 0;
        for voter in self.votes.keys() {
            if let Some(info) = set.get(voter) {
                if !info.jailed {
                    sum = sum.saturating_add(info.voting_power);
                }
            }
        }
        sum
    }

    /// Compute the current `FinalityStatus` against `set`. Threshold is
    /// `> 2/3 * total_active_power` — re-uses the canonical
    /// `ValidatorSet::quorum_threshold()` so we don't drift if the
    /// rounding rule changes there.
    pub fn status(&self, set: &ValidatorSet) -> FinalityStatus {
        if self.votes.is_empty() {
            return FinalityStatus::Pending;
        }
        let voted = self.voted_power(set);
        if set.is_quorum(voted) {
            FinalityStatus::Finalized
        } else {
            FinalityStatus::Justified
        }
    }
}

impl Encodable for FinalityCheckpoint {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(3);
        s.append(&self.height);
        append_h256(s, &self.block_hash);
        s.begin_list(self.votes.len());
        for v in self.votes.values() {
            s.append(v);
        }
    }
}

impl Decodable for FinalityCheckpoint {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if rlp.item_count()? != 3 { return Err(DecoderError::RlpIncorrectListLen); }
        let height:     u64  = rlp.val_at(0)?;
        let block_hash: H256 = decode_h256(&rlp.at(1)?)?;
        let votes_rlp        = rlp.at(2)?;

        let mut votes = BTreeMap::new();
        for i in 0..votes_rlp.item_count()? {
            let v: FinalityVote = votes_rlp.val_at(i)?;
            if v.height != height {
                return Err(DecoderError::Custom("FinalityCheckpoint: vote height mismatch"));
            }
            if v.block_hash != block_hash {
                return Err(DecoderError::Custom("FinalityCheckpoint: vote hash mismatch"));
            }
            votes.insert(v.voter, v);
        }
        let cp = Self { height, block_hash, votes };
        cp.validate()?;
        Ok(cp)
    }
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
        s.upsert(ValidatorInfo::new(addr(2), 100, false, 0).unwrap()).unwrap();
        s.upsert(ValidatorInfo::new(addr(3), 100, false, 0).unwrap()).unwrap();
        s
    }

    fn vote_from(voter_byte: u8, height: u64, hash: H256) -> FinalityVote {
        FinalityVote::new(addr(voter_byte), height, hash, vec![0xab; 64]).unwrap()
    }

    #[test]
    fn empty_checkpoint_is_pending() {
        let set = three_validator_set();
        let cp = FinalityCheckpoint::new(10, H256::repeat_byte(1));
        assert_eq!(cp.status(&set), FinalityStatus::Pending);
        assert_eq!(cp.voted_power(&set), 0);
    }

    #[test]
    fn one_vote_is_justified_not_finalized() {
        let set = three_validator_set();
        let h = H256::repeat_byte(1);
        let mut cp = FinalityCheckpoint::new(10, h);
        cp.add_vote(vote_from(1, 10, h)).unwrap();
        assert_eq!(cp.status(&set), FinalityStatus::Justified);
        assert_eq!(cp.voted_power(&set), 100);
    }

    #[test]
    fn three_of_three_finalizes_with_equal_stake() {
        // ValidatorSet::quorum_threshold uses strict > 2/3 (i.e. 2*N/3 + 1).
        // For 3 × 100 stake, threshold = 201, so 2/3 = 200 is NOT enough,
        // 3/3 = 300 IS. This pins the threshold semantics and protects
        // against accidental boundary loosening in consensus.rs.
        let set = three_validator_set();
        let h = H256::repeat_byte(1);
        let mut cp = FinalityCheckpoint::new(10, h);
        cp.add_vote(vote_from(1, 10, h)).unwrap();
        cp.add_vote(vote_from(2, 10, h)).unwrap();
        // 200/300 == exactly 2/3 → still Justified, not Finalized.
        assert_eq!(cp.status(&set), FinalityStatus::Justified);
        cp.add_vote(vote_from(3, 10, h)).unwrap();
        assert_eq!(cp.voted_power(&set), 300);
        assert_eq!(cp.status(&set), FinalityStatus::Finalized);
    }

    #[test]
    fn weighted_majority_finalizes_above_threshold() {
        // 1 validator at 700, 2 validators at 100 each → total 900.
        // quorum_threshold = 601. The 700-stake validator alone exceeds it.
        let mut set = ValidatorSet::new();
        set.upsert(ValidatorInfo::new(addr(1), 700, false, 0).unwrap()).unwrap();
        set.upsert(ValidatorInfo::new(addr(2), 100, false, 0).unwrap()).unwrap();
        set.upsert(ValidatorInfo::new(addr(3), 100, false, 0).unwrap()).unwrap();
        let h = H256::repeat_byte(1);
        let mut cp = FinalityCheckpoint::new(10, h);
        cp.add_vote(vote_from(1, 10, h)).unwrap();
        assert_eq!(cp.status(&set), FinalityStatus::Finalized);
    }

    #[test]
    fn jailed_voter_has_zero_weight() {
        let mut set = three_validator_set();
        // Jail validator 2.
        set.upsert(ValidatorInfo::new(addr(2), 100, true, 100).unwrap()).unwrap();
        let h = H256::repeat_byte(1);
        let mut cp = FinalityCheckpoint::new(10, h);
        cp.add_vote(vote_from(1, 10, h)).unwrap();
        cp.add_vote(vote_from(2, 10, h)).unwrap(); // counts for nothing
        assert_eq!(cp.voted_power(&set), 100);
        // Active power = 100+100 = 200; quorum = 2*200/3 + 1 = 134 → Justified only.
        assert_eq!(cp.status(&set), FinalityStatus::Justified);
    }

    #[test]
    fn non_member_voter_ignored() {
        let set = three_validator_set();
        let h = H256::repeat_byte(1);
        let mut cp = FinalityCheckpoint::new(10, h);
        // addr(99) is not in the set.
        cp.add_vote(vote_from(99, 10, h)).unwrap();
        assert_eq!(cp.voted_power(&set), 0);
        // BUT votes is non-empty so status is NOT Pending — it's Justified
        // (a stranger voted, which we *recorded* but assigned 0 weight).
        // We still classify as Justified per the spec: any vote => Justified.
        assert_eq!(cp.status(&set), FinalityStatus::Justified);
    }

    #[test]
    fn duplicate_vote_overwritten() {
        let h = H256::repeat_byte(1);
        let mut cp = FinalityCheckpoint::new(10, h);
        cp.add_vote(FinalityVote::new(addr(1), 10, h, vec![0xaa; 32]).unwrap()).unwrap();
        cp.add_vote(FinalityVote::new(addr(1), 10, h, vec![0xbb; 32]).unwrap()).unwrap();
        assert_eq!(cp.votes.len(), 1);
        assert_eq!(cp.votes[&addr(1)].signature, vec![0xbb; 32]);
    }

    #[test]
    fn add_vote_rejects_height_mismatch() {
        let h = H256::repeat_byte(1);
        let mut cp = FinalityCheckpoint::new(10, h);
        let r = cp.add_vote(FinalityVote::new(addr(1), 11, h, vec![0xaa; 32]).unwrap());
        assert!(r.is_err());
    }

    #[test]
    fn add_vote_rejects_hash_mismatch() {
        let h = H256::repeat_byte(1);
        let other = H256::repeat_byte(2);
        let mut cp = FinalityCheckpoint::new(10, h);
        let r = cp.add_vote(FinalityVote::new(addr(1), 10, other, vec![0xaa; 32]).unwrap());
        assert!(r.is_err());
    }

    #[test]
    fn vote_validate_rejects_empty_sig() {
        let r = FinalityVote::new(addr(1), 10, H256::zero(), vec![]);
        assert!(r.is_err());
    }

    #[test]
    fn vote_validate_rejects_oversize_sig() {
        let big = vec![0u8; FinalityVote::MAX_SIG_LEN + 1];
        let r = FinalityVote::new(addr(1), 10, H256::zero(), big);
        assert!(r.is_err());
    }

    #[test]
    fn rlp_round_trip_vote() {
        let v = vote_from(1, 10, H256::repeat_byte(7));
        let enc = rlp::encode(&v);
        let dec: FinalityVote = rlp::decode(&enc).unwrap();
        assert_eq!(v, dec);
    }

    #[test]
    fn rlp_round_trip_checkpoint() {
        let h = H256::repeat_byte(7);
        let mut cp = FinalityCheckpoint::new(10, h);
        cp.add_vote(vote_from(1, 10, h)).unwrap();
        cp.add_vote(vote_from(2, 10, h)).unwrap();
        cp.add_vote(vote_from(3, 10, h)).unwrap();
        let enc = rlp::encode(&cp);
        let dec: FinalityCheckpoint = rlp::decode(&enc).unwrap();
        assert_eq!(cp, dec);
    }

    #[test]
    fn rlp_round_trip_status() {
        for st in [FinalityStatus::Pending, FinalityStatus::Justified, FinalityStatus::Finalized] {
            let enc = rlp::encode(&st);
            let dec: FinalityStatus = rlp::decode(&enc).unwrap();
            assert_eq!(st, dec);
        }
        assert!(FinalityStatus::from_tag(99).is_err());
    }
}
