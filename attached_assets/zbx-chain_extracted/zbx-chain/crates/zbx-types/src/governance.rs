//! On-chain governance for blockchain upgrades.
//!
//! A minimal but correct upgrade-governance system: validators propose a
//! `(module_name, new_version, activation_block)` tuple, every validator
//! casts a single vote, and once a simple majority of the validator set
//! has voted **Yes** the proposal auto-transitions to **Scheduled** for
//! the runtime to pick up at the activation block.
//!
//! The intent matches Cosmos SDK `x/upgrade` + `x/gov` MVP: the *types*
//! are consensus-canonical and live in `zbx-types`; the *transactions*
//! that mutate them, and the validator-set source-of-truth, live in
//! consumer crates (`zbx-tx`, `zbx-consensus`, `zbx-state`).
//!
//! ## Status machine
//!
//! ```text
//!                ┌─── tally < majority ──> Rejected
//! Pending ──────┤
//!                └─── tally ≥ majority ──> Scheduled ──> Executed
//!                                                 │
//!                                                 └────> Failed
//! ```
//!
//! Once a proposal is **non-Pending** it is immutable except for the
//! Scheduled → Executed / Failed terminal transition (set by the
//! migration pipeline after the activation block).
//!
//! ## Invariants (enforced on construct + serde decode + rlp decode)
//! * `module_name` matches `[a-z0-9_-]+` (shared name policy with
//!   `module_version`, `feature_flags`, `activation`).
//! * `votes` is sorted by `Address` (canonical wire form). RLP decode
//!   rejects unsorted / duplicate voter rows.
//! * `Vote` and `ProposalStatus` are encoded as a single byte each;
//!   decode rejects any out-of-range tag so two semantically-equal
//!   proposals always have the same `keccak256(rlp(p))`.
//! * `ProposalRegistry` is keyed by `ProposalId` and serialised as a
//!   strictly-sorted list (no duplicate IDs on decode).
//! * `cast_vote` is the only mutator while a proposal is `Pending`.
//!   Status transitions go through `try_finalize` and the terminal
//!   helpers `mark_executed` / `mark_failed`.

use crate::address::Address;
use crate::module_version::ModuleVersion;
use crate::ZbxError;
use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

// ----------------------------------------------------------------------------
// Name validation (shared discipline)
// ----------------------------------------------------------------------------

fn validate_module_name(name: &str) -> Result<(), ZbxError> {
    if name.is_empty() {
        return Err(ZbxError::InvalidInput(
            "UpgradeProposal.module_name is empty".into(),
        ));
    }
    if !name
        .bytes()
        .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'))
    {
        return Err(ZbxError::InvalidInput(format!(
            "UpgradeProposal.module_name {name:?} must match [a-z0-9_-]+"
        )));
    }
    Ok(())
}

// ----------------------------------------------------------------------------
// ProposalId — opaque u64 newtype
// ----------------------------------------------------------------------------

/// Monotonic on-chain proposal identifier.
///
/// Allocated by the governance pallet at proposal-submission time;
/// stored in the world-state trie keyed by `ProposalId`. Wraps a `u64`
/// so two billion proposals can fit in canonical RLP without ever
/// needing big-int handling.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct ProposalId(pub u64);

impl ProposalId {
    /// Reserved sentinel: governance allocators MUST start from `1`.
    pub const ZERO: ProposalId = ProposalId(0);

    /// Next consecutive id (saturating; governance is rate-limited so
    /// `u64::MAX` is unreachable in practice but cannot panic).
    pub fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

impl fmt::Display for ProposalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

impl Encodable for ProposalId {
    fn rlp_append(&self, s: &mut RlpStream) {
        // Direct delegation — `s.append(&self.0)` would double-count the
        // inner `note_appended(1)` against the outer parent list.
        self.0.rlp_append(s);
    }
}

impl Decodable for ProposalId {
    fn decode(rlp: &Rlp<'_>) -> Result<Self, DecoderError> {
        Ok(Self(rlp.as_val()?))
    }
}

// ----------------------------------------------------------------------------
// Vote — one byte enum
// ----------------------------------------------------------------------------

/// A validator's stance on an `UpgradeProposal`.
///
/// Encoded as a single byte (`Yes=0`, `No=1`, `Abstain=2`); RLP /
/// serde decode rejects any other value so a tampered tally cannot
/// smuggle in a fourth variant. `Abstain` counts toward quorum but
/// not toward the Yes-share majority threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Vote {
    Yes,
    No,
    Abstain,
}

impl Vote {
    fn to_u8(self) -> u8 {
        match self {
            Vote::Yes => 0,
            Vote::No => 1,
            Vote::Abstain => 2,
        }
    }

    fn from_u8(b: u8) -> Result<Self, ZbxError> {
        match b {
            0 => Ok(Vote::Yes),
            1 => Ok(Vote::No),
            2 => Ok(Vote::Abstain),
            other => Err(ZbxError::InvalidInput(format!(
                "Vote tag {other} out of range (0=Yes, 1=No, 2=Abstain)"
            ))),
        }
    }
}

impl fmt::Display for Vote {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Vote::Yes => "yes",
            Vote::No => "no",
            Vote::Abstain => "abstain",
        })
    }
}

impl Encodable for Vote {
    fn rlp_append(&self, s: &mut RlpStream) {
        // Direct delegation; see `ProposalId::rlp_append` for the
        // double-count rationale.
        self.to_u8().rlp_append(s);
    }
}

impl Decodable for Vote {
    fn decode(rlp: &Rlp<'_>) -> Result<Self, DecoderError> {
        let b: u8 = rlp.as_val()?;
        Vote::from_u8(b).map_err(|_| DecoderError::Custom("Vote tag out of range"))
    }
}

// ----------------------------------------------------------------------------
// ProposalStatus — one byte enum
// ----------------------------------------------------------------------------

/// Lifecycle state of an `UpgradeProposal`.
///
/// Encoded as a single byte (`Pending=0`, `Approved=1`, `Rejected=2`,
/// `Scheduled=3`, `Executed=4`, `Failed=5`); decode rejects any other
/// value. The runtime invariant is documented in the module-level
/// status-machine diagram.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProposalStatus {
    Pending,
    Approved,
    Rejected,
    Scheduled,
    Executed,
    Failed,
}

impl ProposalStatus {
    fn to_u8(self) -> u8 {
        match self {
            ProposalStatus::Pending => 0,
            ProposalStatus::Approved => 1,
            ProposalStatus::Rejected => 2,
            ProposalStatus::Scheduled => 3,
            ProposalStatus::Executed => 4,
            ProposalStatus::Failed => 5,
        }
    }

    fn from_u8(b: u8) -> Result<Self, ZbxError> {
        match b {
            0 => Ok(ProposalStatus::Pending),
            1 => Ok(ProposalStatus::Approved),
            2 => Ok(ProposalStatus::Rejected),
            3 => Ok(ProposalStatus::Scheduled),
            4 => Ok(ProposalStatus::Executed),
            5 => Ok(ProposalStatus::Failed),
            other => Err(ZbxError::InvalidInput(format!(
                "ProposalStatus tag {other} out of range (0..=5)"
            ))),
        }
    }

    /// True if the proposal is still accepting votes.
    pub fn is_pending(self) -> bool {
        matches!(self, ProposalStatus::Pending)
    }

    /// True if the proposal has reached a permanent terminal state
    /// (Executed / Failed / Rejected).
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            ProposalStatus::Executed | ProposalStatus::Failed | ProposalStatus::Rejected
        )
    }
}

impl fmt::Display for ProposalStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ProposalStatus::Pending => "pending",
            ProposalStatus::Approved => "approved",
            ProposalStatus::Rejected => "rejected",
            ProposalStatus::Scheduled => "scheduled",
            ProposalStatus::Executed => "executed",
            ProposalStatus::Failed => "failed",
        })
    }
}

impl Encodable for ProposalStatus {
    fn rlp_append(&self, s: &mut RlpStream) {
        // Direct delegation; see `ProposalId::rlp_append` for the
        // double-count rationale.
        self.to_u8().rlp_append(s);
    }
}

impl Decodable for ProposalStatus {
    fn decode(rlp: &Rlp<'_>) -> Result<Self, DecoderError> {
        let b: u8 = rlp.as_val()?;
        ProposalStatus::from_u8(b)
            .map_err(|_| DecoderError::Custom("ProposalStatus tag out of range"))
    }
}

// ----------------------------------------------------------------------------
// VoteTally — counts produced by `tally`
// ----------------------------------------------------------------------------

/// Aggregate vote counts from `UpgradeProposal::tally`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VoteTally {
    pub yes: u32,
    pub no: u32,
    pub abstain: u32,
    /// Total active validators in the set this tally was taken against.
    pub validator_set_size: u32,
}

impl VoteTally {
    /// True when `yes` > 50 % of the active validator set.
    ///
    /// Absentees and abstentions count effectively as "no" — this is
    /// the standard interpretation of "basic majority" and matches
    /// the spec's MVP brief.
    pub fn has_majority(&self) -> bool {
        // Use 64-bit arithmetic to avoid u32 overflow with large sets.
        let yes = self.yes as u64;
        let total = self.validator_set_size as u64;
        yes * 2 > total
    }
}

// ----------------------------------------------------------------------------
// Address RLP helpers (Address itself doesn't impl Encodable/Decodable)
// ----------------------------------------------------------------------------

fn append_address(s: &mut RlpStream, a: &Address) {
    s.append(&a.0.as_slice());
}

fn decode_address(rlp: &Rlp<'_>) -> Result<Address, DecoderError> {
    let bytes: Vec<u8> = rlp.as_val()?;
    if bytes.len() != 20 {
        return Err(DecoderError::Custom("Address must be 20 bytes"));
    }
    let mut arr = [0u8; 20];
    arr.copy_from_slice(&bytes);
    Ok(Address(arr))
}

// ----------------------------------------------------------------------------
// UpgradeProposal
// ----------------------------------------------------------------------------

/// A governance proposal to bump a single module to a new consensus
/// version at a fixed block height.
///
/// Once constructed the body is **immutable except for the `votes`
/// map and the `status` field**. The runtime mutates votes via
/// [`UpgradeProposal::cast_vote`] and finalises via
/// [`UpgradeProposal::try_finalize`]; the migration pipeline calls
/// [`UpgradeProposal::mark_executed`] / [`UpgradeProposal::mark_failed`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UpgradeProposal {
    /// Monotonic on-chain id (allocated by the governance pallet).
    pub id: ProposalId,
    /// Module to upgrade (must match `[a-z0-9_-]+`).
    pub module_name: String,
    /// Target consensus version. Must be `>` the currently-active
    /// version at execution time (re-checked by the migration runner;
    /// we validate the *name* only at type-construction).
    pub new_version: u32,
    /// Block at which the upgrade takes effect.
    pub activation_block: u64,
    /// Validator address that submitted the proposal.
    pub proposer: Address,
    /// Cast votes, sorted by validator `Address`.
    pub votes: BTreeMap<Address, Vote>,
    /// Lifecycle state. Always `Pending` at construction.
    pub status: ProposalStatus,
}

#[derive(Deserialize)]
struct UpgradeProposalRaw {
    id: ProposalId,
    module_name: String,
    new_version: u32,
    activation_block: u64,
    proposer: Address,
    votes: BTreeMap<Address, Vote>,
    status: ProposalStatus,
}

impl<'de> Deserialize<'de> for UpgradeProposal {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = UpgradeProposalRaw::deserialize(d)?;
        validate_module_name(&raw.module_name).map_err(serde::de::Error::custom)?;
        Ok(Self {
            id: raw.id,
            module_name: raw.module_name,
            new_version: raw.new_version,
            activation_block: raw.activation_block,
            proposer: raw.proposer,
            votes: raw.votes,
            status: raw.status,
        })
    }
}

impl UpgradeProposal {
    /// Construct a `Pending` proposal. Validates `module_name`.
    pub fn new(
        id: ProposalId,
        module_name: impl Into<String>,
        new_version: u32,
        activation_block: u64,
        proposer: Address,
    ) -> Result<Self, ZbxError> {
        let module_name = module_name.into();
        validate_module_name(&module_name)?;
        Ok(Self {
            id,
            module_name,
            new_version,
            activation_block,
            proposer,
            votes: BTreeMap::new(),
            status: ProposalStatus::Pending,
        })
    }

    /// Convenience: build the corresponding `ModuleVersion` row that
    /// the registry will receive at activation.
    pub fn target_module_version(&self) -> Result<ModuleVersion, ZbxError> {
        ModuleVersion::new(self.module_name.clone(), self.new_version)
    }

    /// Cast or replace `validator`'s vote.
    ///
    /// Rejected when the proposal is no longer `Pending`. Replacing an
    /// existing vote is allowed (validator changes their mind before
    /// the tally) — this matches Cosmos `x/gov` semantics.
    pub fn cast_vote(&mut self, validator: Address, vote: Vote) -> Result<(), ZbxError> {
        if !self.status.is_pending() {
            return Err(ZbxError::InvalidInput(format!(
                "cannot vote on proposal {} in status {}",
                self.id, self.status
            )));
        }
        self.votes.insert(validator, vote);
        Ok(())
    }

    /// Tally the current votes against an external `validator_set`.
    ///
    /// Votes from addresses outside `validator_set` are silently
    /// ignored (they can no longer influence the outcome — e.g. a
    /// validator that left after voting). Absent validators count as
    /// implicit "no" by virtue of `VoteTally::has_majority` measuring
    /// against `validator_set_size`.
    pub fn tally(&self, validator_set: &BTreeSet<Address>) -> VoteTally {
        let mut t = VoteTally {
            validator_set_size: validator_set.len() as u32,
            ..Default::default()
        };
        for (voter, vote) in &self.votes {
            if !validator_set.contains(voter) {
                continue;
            }
            match vote {
                Vote::Yes => t.yes += 1,
                Vote::No => t.no += 1,
                Vote::Abstain => t.abstain += 1,
            }
        }
        t
    }

    /// Finalise a `Pending` proposal against `validator_set`.
    ///
    /// * `Yes` simple majority **and** `activation_block > current_block`
    ///   → status becomes `Scheduled` (auto-scheduled per the spec).
    /// * `Yes` simple majority **but** `activation_block ≤ current_block`
    ///   → status becomes `Rejected` (the proposal was approved too late
    ///   to take effect — fail-closed rather than retroactively activate).
    /// * No majority → status becomes `Rejected`.
    ///
    /// Returns the new status. No-op (returns the existing status) if
    /// the proposal is already finalised — never moves backwards.
    pub fn try_finalize(
        &mut self,
        validator_set: &BTreeSet<Address>,
        current_block: u64,
    ) -> Result<ProposalStatus, ZbxError> {
        if !self.status.is_pending() {
            return Ok(self.status);
        }
        let tally = self.tally(validator_set);
        let new_status = if tally.has_majority() && self.activation_block > current_block {
            ProposalStatus::Scheduled
        } else {
            ProposalStatus::Rejected
        };
        self.status = new_status;
        Ok(new_status)
    }

    /// Terminal: migration succeeded. Requires `Scheduled` status.
    pub fn mark_executed(&mut self) -> Result<(), ZbxError> {
        if self.status != ProposalStatus::Scheduled {
            return Err(ZbxError::InvalidInput(format!(
                "mark_executed requires Scheduled, got {}",
                self.status
            )));
        }
        self.status = ProposalStatus::Executed;
        Ok(())
    }

    /// Terminal: migration failed. Requires `Scheduled` status.
    pub fn mark_failed(&mut self) -> Result<(), ZbxError> {
        if self.status != ProposalStatus::Scheduled {
            return Err(ZbxError::InvalidInput(format!(
                "mark_failed requires Scheduled, got {}",
                self.status
            )));
        }
        self.status = ProposalStatus::Failed;
        Ok(())
    }
}

impl Encodable for UpgradeProposal {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(7);
        s.append(&self.id);
        s.append(&self.module_name);
        s.append(&self.new_version);
        s.append(&self.activation_block);
        append_address(s, &self.proposer);
        // votes as sorted list-of-rows
        s.begin_list(self.votes.len());
        for (voter, vote) in &self.votes {
            s.begin_list(2);
            append_address(s, voter);
            s.append(vote);
        }
        s.append(&self.status);
    }
}

impl Decodable for UpgradeProposal {
    fn decode(rlp: &Rlp<'_>) -> Result<Self, DecoderError> {
        if rlp.item_count()? != 7 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        let id: ProposalId = rlp.val_at(0)?;
        let module_name: String = rlp.val_at(1)?;
        validate_module_name(&module_name)
            .map_err(|_| DecoderError::Custom("UpgradeProposal.module_name invalid"))?;
        let new_version: u32 = rlp.val_at(2)?;
        let activation_block: u64 = rlp.val_at(3)?;
        let proposer = decode_address(&rlp.at(4)?)?;
        let votes_rlp = rlp.at(5)?;
        let mut votes = BTreeMap::new();
        let mut prev: Option<Address> = None;
        for row in votes_rlp.iter() {
            if row.item_count()? != 2 {
                return Err(DecoderError::RlpIncorrectListLen);
            }
            let voter = decode_address(&row.at(0)?)?;
            let vote: Vote = row.val_at(1)?;
            if let Some(p) = &prev {
                if voter.0.as_slice() <= p.0.as_slice() {
                    return Err(DecoderError::Custom(
                        "UpgradeProposal.votes must be strictly sorted by address",
                    ));
                }
            }
            prev = Some(voter);
            votes.insert(voter, vote);
        }
        let status: ProposalStatus = rlp.val_at(6)?;
        Ok(Self {
            id,
            module_name,
            new_version,
            activation_block,
            proposer,
            votes,
            status,
        })
    }
}

// ----------------------------------------------------------------------------
// ProposalRegistry
// ----------------------------------------------------------------------------

/// Canonical registry of every governance proposal, keyed by `ProposalId`
/// and serialised in strictly-ascending id order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ProposalRegistry(BTreeMap<ProposalId, UpgradeProposal>);

impl ProposalRegistry {
    /// Empty registry.
    pub fn new() -> Self {
        Self(BTreeMap::new())
    }

    /// Insert a fresh proposal. Rejects duplicate ids and any proposal
    /// whose `id` does not match the registry key.
    pub fn insert(&mut self, proposal: UpgradeProposal) -> Result<(), ZbxError> {
        if self.0.contains_key(&proposal.id) {
            return Err(ZbxError::InvalidInput(format!(
                "proposal {} already exists",
                proposal.id
            )));
        }
        self.0.insert(proposal.id, proposal);
        Ok(())
    }

    /// Look up a proposal by id.
    pub fn get(&self, id: ProposalId) -> Option<&UpgradeProposal> {
        self.0.get(&id)
    }

    /// Mutable lookup (used by the runtime to cast votes / finalise).
    pub fn get_mut(&mut self, id: ProposalId) -> Option<&mut UpgradeProposal> {
        self.0.get_mut(&id)
    }

    /// True if no proposals exist.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Total proposal count.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Iterate over every proposal in canonical (id-ascending) order.
    pub fn iter(&self) -> impl Iterator<Item = &UpgradeProposal> + '_ {
        self.0.values()
    }

    /// Iterate over every proposal still accepting votes.
    pub fn pending(&self) -> impl Iterator<Item = &UpgradeProposal> + '_ {
        self.0.values().filter(|p| p.status.is_pending())
    }

    /// Iterate over every approved-and-scheduled proposal whose
    /// activation block is `≤ current_block` (i.e. ready to execute).
    pub fn ready_to_execute(&self, current_block: u64) -> impl Iterator<Item = &UpgradeProposal> {
        self.0.values().filter(move |p| {
            p.status == ProposalStatus::Scheduled && p.activation_block <= current_block
        })
    }
}

impl<'de> Deserialize<'de> for ProposalRegistry {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let map = BTreeMap::<ProposalId, UpgradeProposal>::deserialize(d)?;
        // Cross-field invariant: registry key must match proposal.id
        // (otherwise lookup-by-id would silently return mismatched data).
        for (k, v) in &map {
            if *k != v.id {
                return Err(serde::de::Error::custom(format!(
                    "ProposalRegistry key {} does not match proposal.id {}",
                    k, v.id
                )));
            }
        }
        Ok(Self(map))
    }
}

impl Encodable for ProposalRegistry {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(self.0.len());
        for proposal in self.0.values() {
            s.append(proposal);
        }
    }
}

impl Decodable for ProposalRegistry {
    fn decode(rlp: &Rlp<'_>) -> Result<Self, DecoderError> {
        let mut map = BTreeMap::new();
        let mut prev: Option<ProposalId> = None;
        for row in rlp.iter() {
            let proposal: UpgradeProposal = UpgradeProposal::decode(&row)?;
            if let Some(p) = prev {
                if proposal.id <= p {
                    return Err(DecoderError::Custom(
                        "ProposalRegistry rows must be strictly id-ascending",
                    ));
                }
            }
            prev = Some(proposal.id);
            map.insert(proposal.id, proposal);
        }
        Ok(Self(map))
    }
}

// ----------------------------------------------------------------------------
// Tests
// ----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rlp::{decode, encode};

    fn addr(b: u8) -> Address {
        Address([b; 20])
    }

    fn mk(id: u64, name: &str, ver: u32, block: u64) -> UpgradeProposal {
        UpgradeProposal::new(ProposalId(id), name, ver, block, addr(0xAA)).unwrap()
    }

    #[test]
    fn new_validates_module_name() {
        assert!(UpgradeProposal::new(ProposalId(1), "evm", 2, 1000, addr(1)).is_ok());
        assert!(UpgradeProposal::new(ProposalId(1), "EVM", 2, 1000, addr(1)).is_err());
        assert!(UpgradeProposal::new(ProposalId(1), "", 2, 1000, addr(1)).is_err());
        assert!(UpgradeProposal::new(ProposalId(1), "ev m", 2, 1000, addr(1)).is_err());
    }

    #[test]
    fn fresh_proposal_starts_pending_with_no_votes() {
        let p = mk(1, "evm", 2, 1000);
        assert_eq!(p.status, ProposalStatus::Pending);
        assert!(p.votes.is_empty());
        assert_eq!(p.id, ProposalId(1));
    }

    #[test]
    fn cast_vote_inserts_and_replaces() {
        let mut p = mk(1, "evm", 2, 1000);
        p.cast_vote(addr(1), Vote::Yes).unwrap();
        p.cast_vote(addr(2), Vote::No).unwrap();
        assert_eq!(p.votes.len(), 2);
        // Replace
        p.cast_vote(addr(1), Vote::Abstain).unwrap();
        assert_eq!(p.votes[&addr(1)], Vote::Abstain);
    }

    #[test]
    fn cast_vote_rejected_after_finalize() {
        let mut p = mk(1, "evm", 2, 1000);
        p.cast_vote(addr(1), Vote::Yes).unwrap();
        let vset: BTreeSet<_> = [addr(1)].into_iter().collect();
        p.try_finalize(&vset, 100).unwrap();
        assert_eq!(p.status, ProposalStatus::Scheduled);
        assert!(p.cast_vote(addr(2), Vote::Yes).is_err());
    }

    #[test]
    fn tally_ignores_non_validators() {
        let mut p = mk(1, "evm", 2, 1000);
        p.cast_vote(addr(1), Vote::Yes).unwrap();
        p.cast_vote(addr(2), Vote::Yes).unwrap();
        p.cast_vote(addr(99), Vote::Yes).unwrap(); // not in set
        let vset: BTreeSet<_> = [addr(1), addr(2), addr(3)].into_iter().collect();
        let t = p.tally(&vset);
        assert_eq!(t.yes, 2);
        assert_eq!(t.no, 0);
        assert_eq!(t.abstain, 0);
        assert_eq!(t.validator_set_size, 3);
    }

    #[test]
    fn majority_threshold_is_strict() {
        // 2 of 3 = 66 % > 50 % → majority
        let t = VoteTally {
            yes: 2,
            no: 0,
            abstain: 0,
            validator_set_size: 3,
        };
        assert!(t.has_majority());
        // 2 of 4 = 50 % NOT > 50 % → no majority
        let t = VoteTally {
            yes: 2,
            no: 0,
            abstain: 0,
            validator_set_size: 4,
        };
        assert!(!t.has_majority());
        // 3 of 4 = 75 % → majority
        let t = VoteTally {
            yes: 3,
            no: 0,
            abstain: 0,
            validator_set_size: 4,
        };
        assert!(t.has_majority());
    }

    #[test]
    fn finalize_auto_schedules_on_majority() {
        let mut p = mk(1, "evm", 2, 1000);
        p.cast_vote(addr(1), Vote::Yes).unwrap();
        p.cast_vote(addr(2), Vote::Yes).unwrap();
        let vset: BTreeSet<_> = [addr(1), addr(2), addr(3)].into_iter().collect();
        let new = p.try_finalize(&vset, 500).unwrap();
        assert_eq!(new, ProposalStatus::Scheduled);
        assert_eq!(p.status, ProposalStatus::Scheduled);
    }

    #[test]
    fn finalize_rejects_when_below_majority() {
        let mut p = mk(1, "evm", 2, 1000);
        p.cast_vote(addr(1), Vote::Yes).unwrap();
        p.cast_vote(addr(2), Vote::No).unwrap();
        let vset: BTreeSet<_> = [addr(1), addr(2), addr(3)].into_iter().collect();
        let new = p.try_finalize(&vset, 500).unwrap();
        assert_eq!(new, ProposalStatus::Rejected);
    }

    #[test]
    fn finalize_rejects_when_activation_block_already_passed() {
        let mut p = mk(1, "evm", 2, 100);
        p.cast_vote(addr(1), Vote::Yes).unwrap();
        p.cast_vote(addr(2), Vote::Yes).unwrap();
        let vset: BTreeSet<_> = [addr(1), addr(2)].into_iter().collect();
        // Majority but activation_block (100) <= current_block (200) → Rejected
        let new = p.try_finalize(&vset, 200).unwrap();
        assert_eq!(new, ProposalStatus::Rejected);
    }

    #[test]
    fn finalize_is_idempotent_after_terminal() {
        let mut p = mk(1, "evm", 2, 100);
        p.cast_vote(addr(1), Vote::No).unwrap();
        let vset: BTreeSet<_> = [addr(1)].into_iter().collect();
        p.try_finalize(&vset, 50).unwrap();
        assert_eq!(p.status, ProposalStatus::Rejected);
        // Re-finalising never moves backwards
        let new = p.try_finalize(&vset, 50).unwrap();
        assert_eq!(new, ProposalStatus::Rejected);
    }

    #[test]
    fn mark_executed_requires_scheduled() {
        let mut p = mk(1, "evm", 2, 1000);
        assert!(p.mark_executed().is_err()); // Pending
        p.cast_vote(addr(1), Vote::Yes).unwrap();
        let vset: BTreeSet<_> = [addr(1)].into_iter().collect();
        p.try_finalize(&vset, 500).unwrap();
        assert_eq!(p.status, ProposalStatus::Scheduled);
        p.mark_executed().unwrap();
        assert_eq!(p.status, ProposalStatus::Executed);
        assert!(p.mark_executed().is_err()); // Already terminal
    }

    #[test]
    fn mark_failed_requires_scheduled() {
        let mut p = mk(1, "evm", 2, 1000);
        p.cast_vote(addr(1), Vote::Yes).unwrap();
        let vset: BTreeSet<_> = [addr(1)].into_iter().collect();
        p.try_finalize(&vset, 500).unwrap();
        p.mark_failed().unwrap();
        assert_eq!(p.status, ProposalStatus::Failed);
        assert!(p.mark_executed().is_err());
    }

    #[test]
    fn rlp_round_trip_full_proposal() {
        let mut p = mk(42, "evm", 7, 9_999);
        p.cast_vote(addr(0x10), Vote::Yes).unwrap();
        p.cast_vote(addr(0x20), Vote::No).unwrap();
        p.cast_vote(addr(0x30), Vote::Abstain).unwrap();
        let bytes = encode(&p);
        let back: UpgradeProposal = decode(&bytes).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn rlp_decode_rejects_invalid_module_name() {
        let mut p = mk(1, "evm", 2, 1000);
        // Tamper post-construction (skip the `new` validator)
        p.module_name = "EVM".into();
        let bytes = encode(&p);
        let err = decode::<UpgradeProposal>(&bytes).unwrap_err();
        assert!(matches!(err, DecoderError::Custom(_)));
    }

    #[test]
    fn rlp_decode_rejects_invalid_vote_tag() {
        // Hand-craft an RLP byte stream where one vote tag is 0x05.
        let mut p = mk(1, "evm", 2, 1000);
        p.cast_vote(addr(1), Vote::Yes).unwrap();
        let bytes = encode(&p);
        // Find and corrupt the vote byte (last byte of vote row before status).
        // Easier: build a deliberately-bad fixture.
        let bad_vote = {
            let mut s = RlpStream::new_list(7);
            s.append(&ProposalId(1));
            s.append(&"evm".to_string());
            s.append(&2u32);
            s.append(&1000u64);
            s.append(&addr(0xAA).0.as_slice());
            s.begin_list(1);
            s.begin_list(2);
            s.append(&addr(1).0.as_slice());
            s.append(&5u8); // INVALID vote tag
            s.append(&ProposalStatus::Pending);
            s.out().to_vec()
        };
        let err = decode::<UpgradeProposal>(&bad_vote).unwrap_err();
        assert!(matches!(err, DecoderError::Custom(_)));
        // sanity: original encoding decodes fine
        let back: UpgradeProposal = decode(&bytes).unwrap();
        assert_eq!(back.votes[&addr(1)], Vote::Yes);
    }

    #[test]
    fn rlp_decode_rejects_unsorted_voters() {
        let bad = {
            let mut s = RlpStream::new_list(7);
            s.append(&ProposalId(1));
            s.append(&"evm".to_string());
            s.append(&2u32);
            s.append(&1000u64);
            s.append(&addr(0xAA).0.as_slice());
            s.begin_list(2);
            // Out-of-order: 0x20 then 0x10
            s.begin_list(2);
            s.append(&addr(0x20).0.as_slice());
            s.append(&Vote::Yes);
            s.begin_list(2);
            s.append(&addr(0x10).0.as_slice());
            s.append(&Vote::Yes);
            s.append(&ProposalStatus::Pending);
            s.out().to_vec()
        };
        let err = decode::<UpgradeProposal>(&bad).unwrap_err();
        assert!(matches!(err, DecoderError::Custom(_)));
    }

    #[test]
    fn rlp_decode_rejects_invalid_status_tag() {
        let bad = {
            let mut s = RlpStream::new_list(7);
            s.append(&ProposalId(1));
            s.append(&"evm".to_string());
            s.append(&2u32);
            s.append(&1000u64);
            s.append(&addr(0xAA).0.as_slice());
            s.begin_list(0);
            s.append(&99u8); // INVALID status tag
            s.out().to_vec()
        };
        let err = decode::<UpgradeProposal>(&bad).unwrap_err();
        assert!(matches!(err, DecoderError::Custom(_)));
    }

    #[test]
    fn json_round_trip_full_proposal() {
        let mut p = mk(42, "zvm", 9, 12345);
        p.cast_vote(addr(7), Vote::Yes).unwrap();
        p.cast_vote(addr(8), Vote::Abstain).unwrap();
        let s = serde_json::to_string(&p).unwrap();
        let back: UpgradeProposal = serde_json::from_str(&s).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn json_deserialize_validates_module_name() {
        let bad = r#"{
            "id": 1,
            "module_name": "EVM",
            "new_version": 2,
            "activation_block": 1000,
            "proposer": "0x0000000000000000000000000000000000000001",
            "votes": {},
            "status": "pending"
        }"#;
        assert!(serde_json::from_str::<UpgradeProposal>(bad).is_err());
    }

    #[test]
    fn registry_insert_rejects_duplicate_id() {
        let mut reg = ProposalRegistry::new();
        reg.insert(mk(1, "evm", 2, 1000)).unwrap();
        assert!(reg.insert(mk(1, "zvm", 3, 2000)).is_err());
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn registry_iter_is_id_ascending() {
        let mut reg = ProposalRegistry::new();
        reg.insert(mk(3, "evm", 2, 1000)).unwrap();
        reg.insert(mk(1, "zvm", 3, 2000)).unwrap();
        reg.insert(mk(2, "da", 4, 3000)).unwrap();
        let ids: Vec<_> = reg.iter().map(|p| p.id).collect();
        assert_eq!(ids, vec![ProposalId(1), ProposalId(2), ProposalId(3)]);
    }

    #[test]
    fn registry_pending_and_ready_filters() {
        let mut reg = ProposalRegistry::new();
        let mut p1 = mk(1, "evm", 2, 100);
        p1.cast_vote(addr(1), Vote::Yes).unwrap();
        let vset: BTreeSet<_> = [addr(1)].into_iter().collect();
        p1.try_finalize(&vset, 50).unwrap(); // Scheduled
        reg.insert(p1).unwrap();
        reg.insert(mk(2, "zvm", 3, 2000)).unwrap(); // Pending

        assert_eq!(reg.pending().count(), 1);
        // Ready at block 150 (>= activation 100 of #1)
        assert_eq!(reg.ready_to_execute(150).count(), 1);
        // Not ready at block 50 (< activation 100)
        assert_eq!(reg.ready_to_execute(50).count(), 0);
    }

    #[test]
    fn registry_rlp_round_trip() {
        let mut reg = ProposalRegistry::new();
        reg.insert(mk(1, "evm", 2, 1000)).unwrap();
        let mut p2 = mk(2, "zvm", 3, 2000);
        p2.cast_vote(addr(0x10), Vote::Yes).unwrap();
        p2.cast_vote(addr(0x20), Vote::No).unwrap();
        reg.insert(p2).unwrap();
        let bytes = encode(&reg);
        let back: ProposalRegistry = decode(&bytes).unwrap();
        assert_eq!(reg, back);
    }

    #[test]
    fn registry_json_rejects_key_id_mismatch() {
        // Key is "5" but proposal.id is 7 — must be rejected.
        let bad = r#"{
            "5": {
                "id": 7,
                "module_name": "evm",
                "new_version": 2,
                "activation_block": 1000,
                "proposer": "0x0000000000000000000000000000000000000001",
                "votes": {},
                "status": "pending"
            }
        }"#;
        assert!(serde_json::from_str::<ProposalRegistry>(bad).is_err());
    }

    #[test]
    fn target_module_version_constructs_validated_row() {
        let p = mk(1, "evm", 7, 1000);
        let mv = p.target_module_version().unwrap();
        assert_eq!(mv.module, "evm");
        assert_eq!(mv.version, 7);
    }
}
