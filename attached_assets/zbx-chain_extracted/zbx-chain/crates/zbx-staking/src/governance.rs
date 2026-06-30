//! Governance proposal lifecycle helpers for the staking dispatcher.
//!
//! The canonical on-chain governance state is stored as two bincode-serialised
//! values in the `Metadata` column family of `ZbxDb` — exactly the same keys
//! that `node/src/block_producer.rs` reads when it runs `apply_ready_governance`.
//! By sharing the same storage keys, both the tx-layer (here) and the
//! block-producer hook operate on the same registry without any extra sync.
//!
//! ## Integration map
//!
//! ```text
//! CastVote tx ──► dispatch_staking_tx
//!                    └─► load_proposal_registry
//!                    └─► proposal.cast_vote(voter, vote)
//!                    └─► proposal.try_finalize(active_set, block)  ← THIS FILE
//!                    └─► persist_proposal_registry
//!
//! end-of-block ─► try_finalize_all_pending (pub) ────────────────► THIS FILE
//!
//! block_producer::apply_ready_governance ─► Scheduled → Executed  (unchanged)
//! ```

use crate::error::StakingError;
use crate::validator::ValidatorSet;
use std::collections::BTreeSet;
use tracing::{info, warn};
use zbx_storage::ZbxDb;
use zbx_types::{
    address::Address,
    governance::{ProposalId, ProposalRegistry, ProposalStatus, UpgradeProposal, Vote},
};

// ── Storage keys (must match the constants in node/src/block_producer.rs) ───
pub(crate) const META_PROPOSAL_REGISTRY: &[u8] = b"governance/proposal_registry";

// ── Serialise / deserialise helpers ─────────────────────────────────────────

/// Load the canonical `ProposalRegistry` from the `Metadata` column.
/// Returns an empty registry when no governance proposal has been
/// submitted yet (i.e. at genesis or before the first `ProposeUpgrade` tx).
///
/// Decode failure is **fail-closed**: a corrupted registry must NOT be
/// silently replaced with an empty one — the caller must halt or surface
/// the error.
pub fn load_proposal_registry(db: &ZbxDb) -> Result<ProposalRegistry, StakingError> {
    match db.get_metadata(META_PROPOSAL_REGISTRY) {
        Ok(Some(bytes)) => bincode::deserialize::<ProposalRegistry>(&bytes)
            .map_err(|e| StakingError::Persistence(format!("proposal_registry decode: {e}"))),
        Ok(None) => Ok(ProposalRegistry::new()),
        Err(e) => Err(StakingError::Persistence(format!("proposal_registry read: {e}"))),
    }
}

/// Persist the `ProposalRegistry` to the `Metadata` column.
pub fn persist_proposal_registry(db: &ZbxDb, preg: &ProposalRegistry) -> Result<(), StakingError> {
    let bytes = bincode::serialize(preg)
        .map_err(|e| StakingError::Persistence(format!("proposal_registry encode: {e}")))?;
    db.put_metadata(META_PROPOSAL_REGISTRY, bytes)
        .map_err(|e| StakingError::Persistence(format!("proposal_registry write: {e}")))
}

// ── Proposal constructors ────────────────────────────────────────────────────

/// Build an `UpgradeProposal` from the decoded `ProposeUpgrade` staking tx
/// and insert it into the registry.
///
/// The `proposal_id` MUST have been atomically allocated by the caller
/// (typically via `delta.next_proposal_id`) so no two proposals share an id.
pub fn create_proposal(
    preg: &mut ProposalRegistry,
    proposal_id: u64,
    module_name: String,
    new_version: u32,
    activation_block: u64,
    proposer: Address,
) -> Result<ProposalId, StakingError> {
    let id = ProposalId(proposal_id);
    let proposal = UpgradeProposal::new(id, module_name, new_version, activation_block, proposer)
        .map_err(|e| StakingError::BadPayload(format!("ProposeUpgrade: {e}")))?;
    preg.insert(proposal)
        .map_err(|e| StakingError::BadPayload(format!("ProposeUpgrade insert: {e}")))?;
    Ok(id)
}

// ── Vote casting + immediate finalization ────────────────────────────────────

/// Cast a vote on an existing `Pending` proposal and — if quorum is now
/// reached — immediately attempt to finalize it (`Pending → Scheduled` or
/// `Pending → Rejected`).
///
/// Returns the proposal's status after the operation.
///
/// # Errors
/// - `BadPayload` — proposal not found, or it is no longer `Pending`.
/// - `NotAValidator` — `voter` is not in the active set (enforced by the
///   tx dispatcher before reaching here; re-checked inside `cast_vote`).
pub fn cast_and_maybe_finalize(
    preg: &mut ProposalRegistry,
    proposal_id: u64,
    voter: Address,
    approve: bool,
    vs: &ValidatorSet,
    current_block: u64,
) -> Result<ProposalStatus, StakingError> {
    let id = ProposalId(proposal_id);
    let proposal = preg
        .get_mut(id)
        .ok_or_else(|| StakingError::BadPayload(format!("CastVote: proposal {id} not found")))?;

    // Map bool → typed Vote (Abstain not yet exposed on the tx surface).
    let vote = if approve { Vote::Yes } else { Vote::No };
    proposal
        .cast_vote(voter, vote)
        .map_err(|e| StakingError::BadPayload(format!("CastVote: {e}")))?;

    // Build BTreeSet of active validator addresses for the tally.
    let active: BTreeSet<Address> = vs.active_set.iter().copied().collect();

    let status = proposal
        .try_finalize(&active, current_block)
        .map_err(|e| StakingError::BadPayload(format!("try_finalize: {e}")))?;

    match status {
        ProposalStatus::Scheduled => info!(
            ?id, voter = ?voter, "governance proposal reached majority — Scheduled"
        ),
        ProposalStatus::Rejected => warn!(
            ?id, voter = ?voter, "governance proposal rejected after vote"
        ),
        ProposalStatus::Pending => {} // more votes needed
        other => warn!(?id, status = %other, "unexpected post-vote status"),
    }

    Ok(status)
}

// ── End-of-block sweep ───────────────────────────────────────────────────────

/// **End-of-block tick**: sweep every `Pending` proposal in the registry
/// and attempt to finalize each one.
///
/// This is the complementary half to `apply_ready_governance` in
/// `block_producer.rs`:
/// - `try_finalize_all_pending` handles **`Pending → Scheduled | Rejected`**
///   (at each vote or at end-of-block).
/// - `apply_ready_governance` handles **`Scheduled → Executed | Failed`**
///   (at the activation block).
///
/// Call this once per block, after all staking txs in the block have been
/// dispatched, to catch proposals whose vote count crossed quorum due to
/// multiple `CastVote` txs in the same block.
///
/// Returns `true` iff the registry changed (at least one proposal was
/// promoted or rejected). The caller is responsible for persisting the
/// registry when this returns `true`.
///
/// # Gas cost
/// The sweep is `O(pending_proposals)` — the expected size is bounded by
/// the number of live upgrade proposals at any one time (typically ≤ 10
/// across a real network lifetime). No gas is charged for the sweep itself;
/// it runs as part of the block-finalisation infrastructure (same as
/// `apply_ready_governance`).
pub fn try_finalize_all_pending(
    preg: &mut ProposalRegistry,
    vs: &ValidatorSet,
    current_block: u64,
) -> bool {
    let active: BTreeSet<Address> = vs.active_set.iter().copied().collect();

    // Collect pending proposal ids first to avoid borrow conflicts.
    let pending_ids: Vec<ProposalId> = preg
        .pending()
        .map(|p| p.id)
        .collect();

    if pending_ids.is_empty() {
        return false;
    }

    let mut changed = false;
    for id in pending_ids {
        let Some(proposal) = preg.get_mut(id) else { continue };
        let before = proposal.status;
        match proposal.try_finalize(&active, current_block) {
            Ok(status) if status != before => {
                changed = true;
                match status {
                    ProposalStatus::Scheduled => info!(
                        ?id,
                        module = %proposal.module_name,
                        new_version = proposal.new_version,
                        activation_block = proposal.activation_block,
                        "governance: proposal Scheduled"
                    ),
                    ProposalStatus::Rejected => warn!(
                        ?id,
                        module = %proposal.module_name,
                        "governance: proposal Rejected (no majority)"
                    ),
                    _ => {}
                }
            }
            Ok(_) => {} // still Pending — more votes needed
            Err(e) => {
                // try_finalize only errors if the proposal was already
                // non-Pending; that cannot happen here since we filtered
                // on pending() above — log and skip defensively.
                warn!(?id, error = %e, "governance try_finalize unexpected error");
            }
        }
    }

    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_addr(b: u8) -> Address {
        Address([b; 20])
    }

    fn make_vs(active: &[u8]) -> ValidatorSet {
        let mut vs = ValidatorSet::new();
        for &b in active {
            vs.active_set.push(make_addr(b));
        }
        vs
    }

    #[test]
    fn cast_and_finalize_majority_schedules() {
        let mut preg = ProposalRegistry::new();
        let vs = make_vs(&[1, 2, 3]);
        let proposer = make_addr(1);

        create_proposal(&mut preg, 1, "zbx-vm".into(), 2, 1_000_000, proposer).unwrap();

        // Two yes votes out of three validators → majority
        let s1 = cast_and_maybe_finalize(&mut preg, 1, make_addr(1), true, &vs, 100).unwrap();
        assert_eq!(s1, ProposalStatus::Pending, "one vote is not enough");

        let s2 = cast_and_maybe_finalize(&mut preg, 1, make_addr(2), true, &vs, 100).unwrap();
        assert_eq!(s2, ProposalStatus::Scheduled, "majority reached → Scheduled");
    }

    #[test]
    fn activation_in_past_rejects() {
        let mut preg = ProposalRegistry::new();
        let vs = make_vs(&[1, 2, 3]);
        let proposer = make_addr(1);

        // activation_block = 50, current_block = 100 → past
        create_proposal(&mut preg, 1, "zbx-vm".into(), 2, 50, proposer).unwrap();

        cast_and_maybe_finalize(&mut preg, 1, make_addr(1), true, &vs, 100).unwrap();
        let s = cast_and_maybe_finalize(&mut preg, 1, make_addr(2), true, &vs, 100).unwrap();
        assert_eq!(s, ProposalStatus::Rejected, "activation in past → Rejected");
    }

    #[test]
    fn no_majority_leaves_pending() {
        let mut preg = ProposalRegistry::new();
        let vs = make_vs(&[1, 2, 3]);
        let proposer = make_addr(1);

        create_proposal(&mut preg, 1, "zbx-vm".into(), 2, 1_000_000, proposer).unwrap();

        let s = cast_and_maybe_finalize(&mut preg, 1, make_addr(1), true, &vs, 100).unwrap();
        assert_eq!(s, ProposalStatus::Pending, "1/3 yes — still Pending");
    }

    #[test]
    fn try_finalize_sweep_promotes_proposals() {
        let mut preg = ProposalRegistry::new();
        let vs = make_vs(&[1, 2, 3]);
        let proposer = make_addr(1);

        // Create two proposals
        create_proposal(&mut preg, 1, "zbx-vm".into(), 2, 1_000_000, proposer).unwrap();
        create_proposal(&mut preg, 2, "zbx-types".into(), 4, 2_000_000, proposer).unwrap();

        // Manually insert majority votes without going through cast_and_maybe_finalize
        // (simulates multiple CastVote txs landing before the sweep)
        preg.get_mut(ProposalId(1)).unwrap().cast_vote(make_addr(1), Vote::Yes).unwrap();
        preg.get_mut(ProposalId(1)).unwrap().cast_vote(make_addr(2), Vote::Yes).unwrap();

        let changed = try_finalize_all_pending(&mut preg, &vs, 100);
        assert!(changed, "sweep should have promoted at least one proposal");
        assert_eq!(preg.get(ProposalId(1)).unwrap().status, ProposalStatus::Scheduled);
        // proposal 2 has no votes yet — still Pending
        assert_eq!(preg.get(ProposalId(2)).unwrap().status, ProposalStatus::Pending);
    }

    #[test]
    fn double_vote_updates_existing() {
        let mut preg = ProposalRegistry::new();
        let vs = make_vs(&[1, 2, 3]);
        let proposer = make_addr(1);
        create_proposal(&mut preg, 1, "zbx-vm".into(), 2, 1_000_000, proposer).unwrap();

        // Vote no, then change to yes — proposal should remain Pending until majority
        cast_and_maybe_finalize(&mut preg, 1, make_addr(1), false, &vs, 100).unwrap();
        let s = cast_and_maybe_finalize(&mut preg, 1, make_addr(1), true, &vs, 100).unwrap();
        // Still Pending — only 1 distinct voter
        assert_eq!(s, ProposalStatus::Pending);
    }
}
