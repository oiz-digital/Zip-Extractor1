//! ZBX on-chain governance — proposal lifecycle.
//!
//! # Governance Flow
//!
//! ```
//! create_proposal(calldata) → ProposalCreated event
//!   ↓  [VOTING_PERIOD = 7 days]
//! vote(proposal_id, For/Against/Abstain)
//!   ↓  [if quorum & majority reached]
//! queue(proposal_id)        → enters TimelockQueue (2 day delay)
//!   ↓  [TIMELOCK_DELAY = 2 days]
//! execute_proposal(proposal_id) → calls target contract on-chain
//! ```
//!
//! # Quorum
//! 10% of total staked ZBX must vote For the proposal.
//! Simple majority (>50% of votes cast) required to pass.
//!
//! # Who Can Propose?
//! Any address with ≥1,000 ZBX staked (anti-spam).
//!
//! # Veto
//! Guardian multisig can veto any proposal before execution.

use std::collections::HashMap;
use serde::{Serialize, Deserialize};

/// Voting period: 7 days in blocks (7 × 43200)
pub const VOTING_PERIOD_BLOCKS: u64 = 302_400;

/// Timelock delay before execution: 2 days
pub const TIMELOCK_DELAY_BLOCKS: u64 = 86_400;

/// Quorum: 10% of total staked ZBX must vote For
pub const QUORUM_BPS: u64 = 1_000; // 10%

/// Min stake to create a proposal: 1,000 ZBX
pub const MIN_PROPOSER_STAKE_WEI: u128 = 1_000 * 1_000_000_000_000_000_000u128;

/// Governance proposal state machine.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ProposalState {
    /// Accepting votes
    Active { end_block: u64 },
    /// Voting closed, did not meet quorum or majority
    Defeated,
    /// Voting passed, waiting for timelock
    Queued { execute_after: u64 },
    /// Successfully executed on-chain
    Executed { at_block: u64 },
    /// Vetoed by guardian before execution
    Vetoed { reason: String },
    /// Expired (passed but not executed before deadline)
    Expired,
}

/// Vote direction.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum VoteDirection {
    For,
    Against,
    Abstain,
}

/// A governance proposal.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Proposal {
    /// Unique proposal ID (sequential)
    pub id:            u64,
    /// Proposer address
    pub proposer:      [u8; 20],
    /// Human-readable title
    pub title:         String,
    /// Proposal description (markdown)
    pub description:   String,
    /// ABI-encoded calldata to execute on target
    pub calldata:      Vec<u8>,
    /// Target contract address (or system precompile)
    pub target:        [u8; 20],
    /// Current state
    pub state:         ProposalState,
    /// Block at which proposal was created
    pub created_at:    u64,
    /// Votes: For / Against / Abstain (in ZBX-wei of voting power)
    pub votes_for:     u128,
    pub votes_against: u128,
    pub votes_abstain: u128,
    /// Per-voter record (prevents double voting)
    pub voted:         HashMap<[u8; 20], VoteDirection>,
}

impl Proposal {
    /// Has this proposal reached quorum?
    pub fn has_quorum(&self, total_staked: u128) -> bool {
        let required = total_staked * QUORUM_BPS as u128 / 10_000;
        self.votes_for >= required
    }

    /// Has this proposal passed (quorum + simple majority)?
    pub fn has_passed(&self, total_staked: u128) -> bool {
        self.has_quorum(total_staked) && self.votes_for > self.votes_against
    }
}

/// ZBX Governance engine (state, held in the admin crate).
pub struct Governance {
    /// All proposals (by ID)
    pub proposals:     HashMap<u64, Proposal>,
    /// Next proposal ID
    pub next_id:       u64,
    /// Guardian multisig (can veto)
    pub guardian:      [u8; 20],
    /// Total ZBX staked at last check (for quorum)
    pub total_staked:  u128,
}

impl Governance {
    pub fn new(guardian: [u8; 20], total_staked: u128) -> Self {
        Self { proposals: HashMap::new(), next_id: 1, guardian, total_staked }
    }

    /// Create a new governance proposal.
    ///
    /// Proposer must have ≥1,000 ZBX staked.
    pub fn create_proposal(
        &mut self,
        proposer:      [u8; 20],
        proposer_stake: u128,
        title:         String,
        description:   String,
        target:        [u8; 20],
        calldata:      Vec<u8>,
        current_block: u64,
    ) -> Result<u64, GovernanceError> {
        if proposer_stake < MIN_PROPOSER_STAKE_WEI {
            return Err(GovernanceError::InsufficientStakeToPropose {
                have: proposer_stake,
                need: MIN_PROPOSER_STAKE_WEI,
            });
        }
        if title.is_empty() || title.len() > 256 {
            return Err(GovernanceError::InvalidTitle);
        }

        let id = self.next_id;
        self.next_id += 1;

        let proposal = Proposal {
            id,
            proposer,
            title:         title.clone(),
            description,
            calldata,
            target,
            state:         ProposalState::Active {
                end_block: current_block + VOTING_PERIOD_BLOCKS,
            },
            created_at:    current_block,
            votes_for:     0,
            votes_against: 0,
            votes_abstain: 0,
            voted:         HashMap::new(),
        };

        self.proposals.insert(id, proposal);

        tracing::info!(
            id       = id,
            proposer = hex::encode(proposer),
            title    = title,
            "Governance proposal created"
        );

        Ok(id)
    }

    /// Cast a vote on an active proposal.
    pub fn cast_vote(
        &mut self,
        voter:          [u8; 20],
        voter_stake:    u128,
        proposal_id:    u64,
        direction:      VoteDirection,
        current_block:  u64,
    ) -> Result<(), GovernanceError> {
        let proposal = self.proposals.get_mut(&proposal_id)
            .ok_or(GovernanceError::ProposalNotFound(proposal_id))?;

        if let ProposalState::Active { end_block } = proposal.state {
            if current_block > end_block {
                return Err(GovernanceError::VotingClosed);
            }
        } else {
            return Err(GovernanceError::VotingClosed);
        }

        if proposal.voted.contains_key(&voter) {
            return Err(GovernanceError::AlreadyVoted);
        }

        proposal.voted.insert(voter, direction);
        match direction {
            VoteDirection::For     => proposal.votes_for     += voter_stake,
            VoteDirection::Against => proposal.votes_against += voter_stake,
            VoteDirection::Abstain => proposal.votes_abstain += voter_stake,
        }

        tracing::info!(
            id     = proposal_id,
            voter  = hex::encode(voter),
            power  = voter_stake,
            dir    = format!("{:?}", direction),
            "Vote cast"
        );
        Ok(())
    }

    /// Finalize voting and queue passed proposals.
    pub fn finalize_voting(
        &mut self,
        proposal_id:   u64,
        current_block: u64,
    ) -> Result<ProposalState, GovernanceError> {
        let proposal = self.proposals.get_mut(&proposal_id)
            .ok_or(GovernanceError::ProposalNotFound(proposal_id))?;

        if let ProposalState::Active { end_block } = proposal.state {
            if current_block <= end_block {
                return Err(GovernanceError::VotingStillActive { remaining: end_block - current_block });
            }
        } else {
            return Err(GovernanceError::AlreadyFinalized);
        }

        let new_state = if proposal.has_passed(self.total_staked) {
            ProposalState::Queued {
                execute_after: current_block + TIMELOCK_DELAY_BLOCKS,
            }
        } else {
            ProposalState::Defeated
        };

        proposal.state = new_state.clone();
        Ok(new_state)
    }

    /// Execute a proposal that has passed voting and timelock.
    ///
    /// This is the on-chain execution step — calls target contract with calldata.
    /// Returns the calldata to be dispatched to the target contract.
    pub fn execute_proposal(
        &mut self,
        proposal_id:   u64,
        current_block: u64,
    ) -> Result<ExecutionReceipt, GovernanceError> {
        let proposal = self.proposals.get_mut(&proposal_id)
            .ok_or(GovernanceError::ProposalNotFound(proposal_id))?;

        let execute_after = match &proposal.state {
            ProposalState::Queued { execute_after } => *execute_after,
            ProposalState::Executed { .. }          => return Err(GovernanceError::AlreadyExecuted),
            ProposalState::Vetoed   { reason }      => return Err(GovernanceError::Vetoed(reason.clone())),
            ProposalState::Defeated                 => return Err(GovernanceError::ProposalDefeated),
            ProposalState::Expired                  => return Err(GovernanceError::ProposalExpired),
            _ => return Err(GovernanceError::NotQueued),
        };

        if current_block < execute_after {
            return Err(GovernanceError::TimelockNotExpired {
                remaining: execute_after - current_block,
            });
        }

        let receipt = ExecutionReceipt {
            proposal_id,
            target:   proposal.target,
            calldata: proposal.calldata.clone(),
            executed_at: current_block,
        };

        proposal.state = ProposalState::Executed { at_block: current_block };

        tracing::info!(
            id     = proposal_id,
            target = hex::encode(proposal.target),
            block  = current_block,
            "Governance proposal executed"
        );

        Ok(receipt)
    }

    /// Guardian veto — prevent execution of a queued proposal.
    pub fn veto_proposal(
        &mut self,
        sender:      [u8; 20],
        proposal_id: u64,
        reason:      String,
    ) -> Result<(), GovernanceError> {
        if sender != self.guardian {
            return Err(GovernanceError::NotGuardian);
        }
        let proposal = self.proposals.get_mut(&proposal_id)
            .ok_or(GovernanceError::ProposalNotFound(proposal_id))?;
        proposal.state = ProposalState::Vetoed { reason };
        Ok(())
    }
}

/// Receipt returned after successful proposal execution.
#[derive(Debug)]
pub struct ExecutionReceipt {
    pub proposal_id:  u64,
    pub target:       [u8; 20],
    pub calldata:     Vec<u8>,
    pub executed_at:  u64,
}

#[derive(Debug, thiserror::Error)]
pub enum GovernanceError {
    #[error("proposal {0} not found")]
    ProposalNotFound(u64),
    #[error("voting period is closed")]
    VotingClosed,
    #[error("voting still active: {remaining} blocks remaining")]
    VotingStillActive { remaining: u64 },
    #[error("already voted on this proposal")]
    AlreadyVoted,
    #[error("proposal already finalized")]
    AlreadyFinalized,
    #[error("proposal already executed")]
    AlreadyExecuted,
    #[error("proposal was vetoed: {0}")]
    Vetoed(String),
    #[error("proposal was defeated")]
    ProposalDefeated,
    #[error("proposal expired")]
    ProposalExpired,
    #[error("proposal is not queued for execution")]
    NotQueued,
    #[error("timelock not expired: {remaining} blocks remaining")]
    TimelockNotExpired { remaining: u64 },
    #[error("not the guardian multisig")]
    NotGuardian,
    #[error("insufficient stake to propose: have {have}, need {need}")]
    InsufficientStakeToPropose { have: u128, need: u128 },
    #[error("invalid proposal title")]
    InvalidTitle,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guardian() -> [u8; 20] { [0xFF; 20] }
    fn proposer() -> [u8; 20] { [0x01; 20] }
    fn voter1()   -> [u8; 20] { [0x10; 20] }
    fn voter2()   -> [u8; 20] { [0x20; 20] }

    const STAKE_1M: u128 = 1_000_000 * 1_000_000_000_000_000_000u128;

    fn gov() -> Governance {
        Governance::new(guardian(), STAKE_1M * 10) // 10M total staked
    }

    #[test]
    fn create_proposal_success() {
        let mut gov = gov();
        let id = gov.create_proposal(
            proposer(), STAKE_1M, // 1M ZBX — above 1000 ZBX min
            "ZEP-013: Increase block gas limit".into(),
            "Proposal to increase MAX_BLOCK_GAS from 30M to 50M".into(),
            [0xC4; 20],
            vec![0x01, 0x02, 0x03],
            100,
        ).unwrap();
        assert_eq!(id, 1);
        assert!(gov.proposals.contains_key(&1));
    }

    #[test]
    fn propose_with_low_stake_rejected() {
        let mut gov = gov();
        let tiny_stake = 100 * 1_000_000_000_000_000_000u128; // 100 ZBX < 1000 min
        let err = gov.create_proposal(
            proposer(), tiny_stake,
            "Bad proposal".into(), "".into(),
            [0x00; 20], vec![],
            100,
        ).unwrap_err();
        assert!(matches!(err, GovernanceError::InsufficientStakeToPropose { .. }));
    }

    #[test]
    fn full_proposal_lifecycle() {
        let mut gov = gov();
        // Total staked = 10M ZBX. Quorum = 10% = 1M ZBX.

        let id = gov.create_proposal(
            proposer(), STAKE_1M,
            "ZEP-013".into(), "Description".into(),
            [0xAA; 20], vec![0xDE, 0xAD],
            0,
        ).unwrap();

        // Vote: voter1 with 2M ZBX For, voter2 with 500k Against
        gov.cast_vote(voter1(), STAKE_1M * 2, id, VoteDirection::For, 1).unwrap();
        gov.cast_vote(voter2(), STAKE_1M / 2, id, VoteDirection::Against, 1).unwrap();

        // Finalize after voting period
        let state = gov.finalize_voting(id, VOTING_PERIOD_BLOCKS + 1).unwrap();
        assert!(matches!(state, ProposalState::Queued { .. }));

        // Execute after timelock
        let receipt = gov.execute_proposal(id, VOTING_PERIOD_BLOCKS + TIMELOCK_DELAY_BLOCKS + 2).unwrap();
        assert_eq!(receipt.proposal_id, id);
        assert_eq!(receipt.target, [0xAA; 20]);
        assert_eq!(receipt.calldata, vec![0xDE, 0xAD]);
    }

    #[test]
    fn double_vote_rejected() {
        let mut gov = gov();
        let id = gov.create_proposal(
            proposer(), STAKE_1M,
            "Test".into(), "".into(), [0x00; 20], vec![], 0,
        ).unwrap();
        gov.cast_vote(voter1(), STAKE_1M, id, VoteDirection::For, 1).unwrap();
        let err = gov.cast_vote(voter1(), STAKE_1M, id, VoteDirection::For, 2).unwrap_err();
        assert!(matches!(err, GovernanceError::AlreadyVoted));
    }

    #[test]
    fn guardian_can_veto() {
        let mut gov = gov();
        let id = gov.create_proposal(
            proposer(), STAKE_1M, "Veto test".into(), "".into(), [0x00; 20], vec![], 0,
        ).unwrap();
        gov.cast_vote(voter1(), STAKE_1M * 5, id, VoteDirection::For, 1).unwrap();
        gov.finalize_voting(id, VOTING_PERIOD_BLOCKS + 1).unwrap();

        gov.veto_proposal(guardian(), id, "Security concern".into()).unwrap();

        let err = gov.execute_proposal(id, VOTING_PERIOD_BLOCKS + TIMELOCK_DELAY_BLOCKS + 2).unwrap_err();
        assert!(matches!(err, GovernanceError::Vetoed(_)));
    }

    #[test]
    fn non_guardian_cannot_veto() {
        let mut gov = gov();
        let id = gov.create_proposal(
            proposer(), STAKE_1M, "Test".into(), "".into(), [0x00; 20], vec![], 0,
        ).unwrap();
        let err = gov.veto_proposal([0x99; 20], id, "".into()).unwrap_err();
        assert!(matches!(err, GovernanceError::NotGuardian));
    }

    #[test]
    fn voting_period_7_days() {
        // 7 days × 86400s/day ÷ 2s/block = 302,400 blocks
        assert_eq!(VOTING_PERIOD_BLOCKS, 302_400);
    }

    #[test]
    fn timelock_2_days() {
        // 2 days × 86400s/day ÷ 2s/block = 86,400 blocks
        assert_eq!(TIMELOCK_DELAY_BLOCKS, 86_400);
    }
}