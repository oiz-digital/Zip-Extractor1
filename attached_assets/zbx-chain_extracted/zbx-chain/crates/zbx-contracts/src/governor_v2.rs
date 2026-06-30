//! GovernorV2 — enhanced on-chain governance with:
//!   * Delegated voting power (ERC-20 Votes style)
//!   * Voting-power snapshot at proposal creation block
//!   * On-chain execution payload (Vec<Call>) queued through TimelockController
//!   * Quorum based on snapshot total supply
//!   * Veto guardian (via TimelockController.cancel)
//!   * States: Pending → Active → Succeeded/Defeated → Queued → Executed | Cancelled

use std::collections::HashMap;
use zbx_types::address::Address;
use crate::timelock::{Call, TimelockController};

pub type ProposalId = u64;

/// Governance proposal lifecycle states.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProposalState {
    /// Created but voting has not started yet (starts at `vote_start` block).
    Pending,
    /// Voting window is open.
    Active { vote_start: u64, vote_end: u64 },
    /// Vote passed — not yet queued in timelock.
    Succeeded,
    /// Vote failed (quorum not reached or against > for).
    Defeated,
    /// Queued in the TimelockController — waiting for delay.
    Queued { timelock_id: [u8; 32] },
    /// Timelock executed — proposal is live on-chain.
    Executed,
    /// Cancelled by guardian veto.
    Cancelled,
}

/// Support type for a governance vote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoteSupport {
    Against = 0,
    For     = 1,
    Abstain = 2,
}

impl TryFrom<u8> for VoteSupport {
    type Error = &'static str;
    fn try_from(v: u8) -> Result<Self, Self::Error> {
        match v {
            0 => Ok(VoteSupport::Against),
            1 => Ok(VoteSupport::For),
            2 => Ok(VoteSupport::Abstain),
            _ => Err("invalid vote support — must be 0 (against), 1 (for), or 2 (abstain)"),
        }
    }
}

/// An on-chain governance proposal.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProposalV2 {
    pub id:            ProposalId,
    pub proposer:      Address,
    pub description:   String,
    /// Block number at which voting power is snapshotted.
    pub snapshot_block: u64,
    pub state:         ProposalState,
    pub votes_for:     u128,
    pub votes_against: u128,
    pub votes_abstain: u128,
    /// Calls to execute if the proposal passes.
    pub calls:         Vec<Call>,
    /// Timelock salt (SHA3 of proposal ID).
    pub salt:          [u8; 32],
}

/// Governance error type.
#[derive(Debug, thiserror::Error)]
pub enum GovError {
    #[error("proposer has insufficient voting power: need {need}, got {got}")]
    InsufficientProposerPower { need: u128, got: u128 },
    #[error("unknown proposal: {0}")]
    UnknownProposal(ProposalId),
    #[error("proposal is not in Active state")]
    NotActive,
    #[error("voter has already voted on proposal {0}")]
    AlreadyVoted(ProposalId),
    #[error("proposal is not in Succeeded state")]
    NotSucceeded,
    #[error("proposal is not in Queued state")]
    NotQueued,
    #[error("proposal is not in Active/Queued state — cannot cancel")]
    CannotCancel,
    #[error("timelock error: {0}")]
    Timelock(String),
    #[error("invalid vote support value")]
    InvalidSupport,
}

/// GovernorV2 state.
pub struct GovernorV2 {
    proposals:       HashMap<ProposalId, ProposalV2>,
    next_id:         ProposalId,
    /// Voting power delegations: delegate → set of delegators (and their power).
    delegations:     HashMap<Address, HashMap<Address, u128>>,
    /// Snapshot of delegated power at proposal creation: proposal_id → (voter → power).
    vote_snapshots:  HashMap<ProposalId, HashMap<Address, u128>>,
    /// Whether a voter has voted: (proposal_id, voter).
    votes_cast:      HashMap<(ProposalId, Address), VoteSupport>,

    // Governance parameters
    /// Minimum voting power required to create a proposal.
    pub proposal_threshold:   u128,
    /// Quorum = fraction of total_supply (bps). Default 400 bps = 4%.
    pub quorum_bps:           u32,
    /// Blocks before voting starts after proposal creation.
    pub voting_delay_blocks:  u64,
    /// Blocks the voting window is open.
    pub voting_period_blocks: u64,
}

impl GovernorV2 {
    pub fn new() -> Self {
        Self {
            proposals:            HashMap::new(),
            next_id:              0,
            delegations:          HashMap::new(),
            vote_snapshots:       HashMap::new(),
            votes_cast:           HashMap::new(),
            proposal_threshold:   100_000 * 1_000_000_000_000_000_000u128, // 100K ZBX
            quorum_bps:           400,    // 4% of total supply
            voting_delay_blocks:  1,
            voting_period_blocks: 43_200, // ~3 days at 6s blocks
        }
    }

    // ── Delegation ────────────────────────────────────────────────────────────

    /// Delegate `amount` voting power from `delegator` to `delegate`.
    /// Replaces any prior delegation from `delegator` to `delegate`.
    pub fn delegate(&mut self, delegator: Address, delegate: Address, amount: u128) {
        self.delegations
            .entry(delegate)
            .or_default()
            .insert(delegator, amount);
    }

    /// Remove delegation from `delegator` to `delegate`.
    pub fn undelegate(&mut self, delegator: Address, delegate: Address) {
        if let Some(inner) = self.delegations.get_mut(&delegate) {
            inner.remove(&delegator);
        }
    }

    /// Compute current voting power of `addr` (sum of all delegated amounts).
    pub fn voting_power(&self, addr: Address) -> u128 {
        self.delegations.get(&addr)
            .map(|m| m.values().sum())
            .unwrap_or(0)
    }

    // ── Snapshot ──────────────────────────────────────────────────────────────

    /// Capture a snapshot of all current voting powers for a new proposal.
    fn snapshot_all(&self) -> HashMap<Address, u128> {
        self.delegations
            .iter()
            .map(|(addr, delegators)| (*addr, delegators.values().sum::<u128>()))
            .collect()
    }

    /// Voting power of `addr` at the snapshot taken for `proposal_id`.
    pub fn votes_at_snapshot(&self, proposal_id: ProposalId, addr: &Address) -> u128 {
        self.vote_snapshots
            .get(&proposal_id)
            .and_then(|snap| snap.get(addr))
            .copied()
            .unwrap_or(0)
    }

    /// Total supply snapshot for quorum calculation.
    fn total_snapshot(&self, proposal_id: ProposalId) -> u128 {
        self.vote_snapshots
            .get(&proposal_id)
            .map(|snap| snap.values().sum())
            .unwrap_or(0)
    }

    // ── Proposal lifecycle ────────────────────────────────────────────────────

    /// Create a new proposal. Proposer must have ≥ `proposal_threshold` voting power.
    pub fn propose(
        &mut self,
        proposer:    Address,
        description: String,
        calls:       Vec<Call>,
        current_block: u64,
    ) -> Result<ProposalId, GovError> {
        let power = self.voting_power(proposer);
        if power < self.proposal_threshold {
            return Err(GovError::InsufficientProposerPower {
                need: self.proposal_threshold,
                got:  power,
            });
        }
        let id = self.next_id;
        self.next_id += 1;

        // Snapshot current voting power for quorum checks.
        let snapshot = self.snapshot_all();
        self.vote_snapshots.insert(id, snapshot);

        let vote_start = current_block + self.voting_delay_blocks;
        let vote_end   = vote_start + self.voting_period_blocks;

        // Derive a deterministic salt from the proposal ID.
        let mut salt = [0u8; 32];
        salt[..8].copy_from_slice(&id.to_be_bytes());

        self.proposals.insert(id, ProposalV2 {
            id,
            proposer,
            description,
            snapshot_block: current_block,
            state: ProposalState::Active { vote_start, vote_end },
            votes_for:     0,
            votes_against: 0,
            votes_abstain: 0,
            calls,
            salt,
        });
        Ok(id)
    }

    /// Cast a vote. Uses snapshot voting power at proposal creation.
    pub fn cast_vote(
        &mut self,
        proposal_id: ProposalId,
        voter:       Address,
        support:     u8,
        current_block: u64,
    ) -> Result<u128, GovError> {
        let support = VoteSupport::try_from(support).map_err(|_| GovError::InvalidSupport)?;

        // Validate state — immutable borrow dropped after this block.
        {
            let prop = self.proposals.get(&proposal_id)
                .ok_or(GovError::UnknownProposal(proposal_id))?;
            if let ProposalState::Active { vote_start, vote_end } = prop.state {
                if current_block < vote_start || current_block > vote_end {
                    return Err(GovError::NotActive);
                }
            } else {
                return Err(GovError::NotActive);
            }
        }

        if self.votes_cast.contains_key(&(proposal_id, voter)) {
            return Err(GovError::AlreadyVoted(proposal_id));
        }

        // votes_at_snapshot borrows vote_snapshots (disjoint from proposals mutable borrow below).
        let weight = self.votes_at_snapshot(proposal_id, &voter);

        let prop = self.proposals.get_mut(&proposal_id).unwrap();
        match support {
            VoteSupport::For     => prop.votes_for     += weight,
            VoteSupport::Against => prop.votes_against += weight,
            VoteSupport::Abstain => prop.votes_abstain += weight,
        }
        self.votes_cast.insert((proposal_id, voter), support);
        Ok(weight)
    }

    /// Finalise a proposal after voting ends. Transitions to Succeeded or Defeated.
    pub fn finalise(
        &mut self,
        proposal_id:    ProposalId,
        current_block:  u64,
    ) -> Result<ProposalState, GovError> {
        // Extract vote tallies under an immutable borrow — dropped before total_snapshot().
        let (votes_for, votes_against, votes_abstain) = {
            let prop = self.proposals.get(&proposal_id)
                .ok_or(GovError::UnknownProposal(proposal_id))?;
            if let ProposalState::Active { vote_end, .. } = prop.state {
                if current_block <= vote_end {
                    return Err(GovError::NotActive);
                }
            } else {
                return Err(GovError::NotActive);
            }
            (prop.votes_for, prop.votes_against, prop.votes_abstain)
        };

        let total_snap = self.total_snapshot(proposal_id);
        let quorum = total_snap * (self.quorum_bps as u128) / 10_000;
        let total_votes = votes_for + votes_against + votes_abstain;

        let new_state = if total_votes >= quorum && votes_for > votes_against {
            ProposalState::Succeeded
        } else {
            ProposalState::Defeated
        };
        self.proposals.get_mut(&proposal_id).unwrap().state = new_state.clone();
        Ok(new_state)
    }

    /// Queue a Succeeded proposal into the TimelockController.
    pub fn queue(
        &mut self,
        proposal_id: ProposalId,
        timelock:    &mut TimelockController,
        queuer:      Address,
        now:         u64,
    ) -> Result<[u8; 32], GovError> {
        let prop = self.proposals.get(&proposal_id)
            .ok_or(GovError::UnknownProposal(proposal_id))?;

        if prop.state != ProposalState::Succeeded {
            return Err(GovError::NotSucceeded);
        }

        let calls = prop.calls.clone();
        let salt  = prop.salt;

        let tl_id = timelock.schedule(queuer, calls, None, salt, now)
            .map_err(|e| GovError::Timelock(e.to_string()))?;

        let prop = self.proposals.get_mut(&proposal_id).unwrap();
        prop.state = ProposalState::Queued { timelock_id: tl_id };
        Ok(tl_id)
    }

    /// Execute a queued proposal through the TimelockController.
    /// Returns the calls to dispatch.
    pub fn execute(
        &mut self,
        proposal_id: ProposalId,
        timelock:    &mut TimelockController,
        now:         u64,
    ) -> Result<Vec<Call>, GovError> {
        let prop = self.proposals.get(&proposal_id)
            .ok_or(GovError::UnknownProposal(proposal_id))?;

        let tl_id = if let ProposalState::Queued { timelock_id } = prop.state {
            timelock_id
        } else {
            return Err(GovError::NotQueued);
        };

        let calls = timelock.execute(tl_id, now)
            .map_err(|e| GovError::Timelock(e.to_string()))?;

        self.proposals.get_mut(&proposal_id).unwrap().state = ProposalState::Executed;
        Ok(calls)
    }

    /// Cancel a queued proposal (guardian veto via timelock).
    pub fn cancel(
        &mut self,
        proposal_id: ProposalId,
        timelock:    &mut TimelockController,
        guardian:    Address,
    ) -> Result<(), GovError> {
        let prop = self.proposals.get(&proposal_id)
            .ok_or(GovError::UnknownProposal(proposal_id))?;

        let tl_id = if let ProposalState::Queued { timelock_id } = prop.state {
            timelock_id
        } else {
            return Err(GovError::CannotCancel);
        };

        timelock.cancel(tl_id, guardian)
            .map_err(|e| GovError::Timelock(e.to_string()))?;

        self.proposals.get_mut(&proposal_id).unwrap().state = ProposalState::Cancelled;
        Ok(())
    }

    pub fn proposal(&self, id: ProposalId) -> Option<&ProposalV2> {
        self.proposals.get(&id)
    }
}

impl Default for GovernorV2 {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timelock::{TimelockController, MIN_DELAY_SECS};

    fn addr(v: u8) -> Address { Address([v; 20]) }

    fn governor_with_voter(power: u128) -> GovernorV2 {
        let mut gov = GovernorV2::new();
        gov.proposal_threshold = power;
        gov.voting_delay_blocks = 0;
        gov.delegate(addr(1), addr(1), power + 1);
        gov
    }

    #[test]
    fn full_proposal_lifecycle() {
        let mut gov = governor_with_voter(100);
        let id = gov.propose(addr(1), "test".into(), vec![], 0).unwrap();
        gov.cast_vote(id, addr(1), 1, 1).unwrap();
        let state = gov.finalise(id, gov.voting_period_blocks + 2).unwrap();
        assert_eq!(state, ProposalState::Succeeded);
    }

    #[test]
    fn defeated_when_quorum_not_reached() {
        let mut gov = GovernorV2::new();
        gov.voting_delay_blocks = 0;
        // Give proposer enough power to propose but not reach quorum
        gov.delegate(addr(1), addr(1), gov.proposal_threshold + 1);
        let id = gov.propose(addr(1), "fail".into(), vec![], 0).unwrap();
        // No votes cast → quorum not reached
        let state = gov.finalise(id, gov.voting_period_blocks + 2).unwrap();
        assert_eq!(state, ProposalState::Defeated);
    }

    #[test]
    fn queue_and_execute_via_timelock() {
        let mut gov = governor_with_voter(100);
        let mut tl = TimelockController::new(MIN_DELAY_SECS, addr(99), addr(0)).unwrap();
        let id = gov.propose(addr(1), "exec".into(), vec![], 0).unwrap();
        gov.cast_vote(id, addr(1), 1, 1).unwrap();
        gov.finalise(id, gov.voting_period_blocks + 2).unwrap();
        let tl_id = gov.queue(id, &mut tl, addr(1), 0).unwrap();
        assert!(matches!(gov.proposal(id).unwrap().state, ProposalState::Queued { .. }));
        let calls = gov.execute(id, &mut tl, MIN_DELAY_SECS + 1).unwrap();
        assert_eq!(calls.len(), 0);
        assert_eq!(gov.proposal(id).unwrap().state, ProposalState::Executed);
        let _ = tl_id;
    }

    #[test]
    fn guardian_can_cancel_queued() {
        let mut gov = governor_with_voter(100);
        let mut tl = TimelockController::new(MIN_DELAY_SECS, addr(99), addr(0)).unwrap();
        let id = gov.propose(addr(1), "cancel".into(), vec![], 0).unwrap();
        gov.cast_vote(id, addr(1), 1, 1).unwrap();
        gov.finalise(id, gov.voting_period_blocks + 2).unwrap();
        gov.queue(id, &mut tl, addr(1), 0).unwrap();
        gov.cancel(id, &mut tl, addr(99)).unwrap();
        assert_eq!(gov.proposal(id).unwrap().state, ProposalState::Cancelled);
    }

    #[test]
    fn double_vote_rejected() {
        let mut gov = governor_with_voter(100);
        let id = gov.propose(addr(1), "dup".into(), vec![], 0).unwrap();
        gov.cast_vote(id, addr(1), 1, 1).unwrap();
        assert!(matches!(gov.cast_vote(id, addr(1), 1, 1), Err(GovError::AlreadyVoted(_))));
    }

    #[test]
    fn insufficient_proposer_power_rejected() {
        let mut gov = GovernorV2::new();
        let err = gov.propose(addr(1), "low-power".into(), vec![], 0).unwrap_err();
        assert!(matches!(err, GovError::InsufficientProposerPower { .. }));
    }
}
