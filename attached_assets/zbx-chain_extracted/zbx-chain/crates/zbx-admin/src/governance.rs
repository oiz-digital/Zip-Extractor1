//! ZBX on-chain governance — proposal lifecycle.
//!
//! # Governance Flow
//!
//! ```text
//! create_proposal(calldata) → ProposalCreated event
//!   ↓  [VOTING_PERIOD = 7 days ≈ 302,400 blocks at 2 s/block]
//! vote(proposal_id, For/Against/Abstain)
//!   ↓  [if quorum (10% total staked) & simple majority reached]
//! finalize_voting(proposal_id)   → ProposalState::Queued
//!   ↓  [TIMELOCK_DELAY = 2 days ≈ 86,400 blocks]
//! execute_proposal(proposal_id)  → dispatches calldata to target
//! ```
//!
//! # Quorum
//! 10 % of total staked ZBX must vote **For** the proposal.
//! A simple majority (votes_for > votes_against) is required to pass.
//!
//! # Who Can Propose?
//! Any address with ≥ 1,000 ZBX staked (anti-spam threshold).
//!
//! # Veto
//! The guardian multisig can veto any proposal before execution.
//!
//! # Double-Vote Prevention
//! Each address may only vote once per proposal; a second attempt
//! returns [`GovernanceError::AlreadyVoted`].
//!
//! Wired into production by `zbx-admin/src/lib.rs` as `pub mod governance`.
//! Previously lived in `_archive/governance.rs` (dead code since genesis).

use std::collections::HashMap;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Voting period: 7 days × 86 400 s/day ÷ 2 s/block = 302 400 blocks.
pub const VOTING_PERIOD_BLOCKS: u64 = 302_400;

/// Timelock delay before execution: 2 days × 86 400 ÷ 2 = 86 400 blocks.
pub const TIMELOCK_DELAY_BLOCKS: u64 = 86_400;

/// Quorum threshold in basis points (10 % = 1 000 bps).
pub const QUORUM_BPS: u64 = 1_000;

/// Minimum stake required to create a proposal: 1 000 ZBX (in wei).
pub const MIN_PROPOSER_STAKE_WEI: u128 = 1_000 * 1_000_000_000_000_000_000u128;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// State machine for a governance proposal.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ProposalState {
    /// Accepting votes — voting ends at `end_block`.
    Active { end_block: u64 },
    /// Voting closed; did not meet quorum or majority.
    Defeated,
    /// Passed; waiting for the timelock to expire.
    Queued { execute_after: u64 },
    /// Successfully executed on-chain.
    Executed { at_block: u64 },
    /// Vetoed by the guardian multisig before execution.
    Vetoed { reason: String },
    /// Queued proposal was not executed before the grace period expired.
    Expired,
}

/// Direction of a governance vote.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum VoteDirection {
    For,
    Against,
    Abstain,
}

/// An individual governance proposal.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Proposal {
    /// Unique sequential proposal ID.
    pub id:            u64,
    /// Proposer's 20-byte EVM address.
    pub proposer:      [u8; 20],
    /// Human-readable title (1–256 chars).
    pub title:         String,
    /// Markdown description of the proposal.
    pub description:   String,
    /// ABI-encoded calldata to execute on `target` if the proposal passes.
    pub calldata:      Vec<u8>,
    /// Target contract address (or system precompile address).
    pub target:        [u8; 20],
    /// Current proposal state.
    pub state:         ProposalState,
    /// Block at which the proposal was submitted.
    pub created_at:    u64,
    /// Total voting power (ZBX-wei) cast For.
    pub votes_for:     u128,
    /// Total voting power cast Against.
    pub votes_against: u128,
    /// Total voting power cast Abstain.
    pub votes_abstain: u128,
    /// Per-address vote record — prevents double voting.
    pub voted:         HashMap<[u8; 20], VoteDirection>,
}

impl Proposal {
    /// True if this proposal has reached the quorum threshold.
    pub fn has_quorum(&self, total_staked: u128) -> bool {
        let required = total_staked
            .saturating_mul(QUORUM_BPS as u128)
            / 10_000;
        self.votes_for >= required
    }

    /// True if this proposal has both quorum and a simple majority.
    pub fn has_passed(&self, total_staked: u128) -> bool {
        self.has_quorum(total_staked) && self.votes_for > self.votes_against
    }
}

// ---------------------------------------------------------------------------
// Governance engine
// ---------------------------------------------------------------------------

/// ZBX governance engine — holds all proposal state.
///
/// Instantiate once per node and keep it behind a `Mutex` /
/// `RwLock` in the admin service.
pub struct Governance {
    /// All proposals, indexed by ID.
    pub proposals:    HashMap<u64, Proposal>,
    /// Next proposal ID (starts at 1).
    pub next_id:      u64,
    /// Guardian multisig address — the only address that can veto.
    pub guardian:     [u8; 20],
    /// Total ZBX staked at the time of the last quorum snapshot.
    /// Operators must update this via [`Governance::update_total_staked`]
    /// on significant stake changes.
    pub total_staked: u128,
}

impl Governance {
    /// Create a new governance engine.
    ///
    /// # Arguments
    /// * `guardian`      — guardian multisig address.
    /// * `total_staked`  — current total ZBX staked (wei).
    pub fn new(guardian: [u8; 20], total_staked: u128) -> Self {
        Self {
            proposals:    HashMap::new(),
            next_id:      1,
            guardian,
            total_staked,
        }
    }

    /// Refresh the total-staked snapshot used for quorum calculations.
    pub fn update_total_staked(&mut self, total_staked: u128) {
        self.total_staked = total_staked;
    }

    // -----------------------------------------------------------------------
    // Lifecycle
    // -----------------------------------------------------------------------

    /// Submit a new governance proposal.
    ///
    /// # Errors
    /// * [`GovernanceError::InsufficientStakeToPropose`] — proposer stake < 1 000 ZBX.
    /// * [`GovernanceError::InvalidTitle`] — title is empty or longer than 256 chars.
    pub fn create_proposal(
        &mut self,
        proposer:        [u8; 20],
        proposer_stake:  u128,
        title:           String,
        description:     String,
        target:          [u8; 20],
        calldata:        Vec<u8>,
        current_block:   u64,
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
            title    = %title,
            "Governance: proposal created"
        );

        Ok(id)
    }

    /// Cast a vote on an active proposal.
    ///
    /// `voter_stake` is the voter's ZBX staking balance in wei;
    /// this is the voting power applied to the tally.
    ///
    /// # Errors
    /// * [`GovernanceError::ProposalNotFound`] — unknown ID.
    /// * [`GovernanceError::VotingClosed`] — proposal is not in `Active` state
    ///   or the voting period has elapsed.
    /// * [`GovernanceError::AlreadyVoted`] — this address already voted.
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

        match proposal.state {
            ProposalState::Active { end_block } if current_block <= end_block => {}
            ProposalState::Active { .. } => return Err(GovernanceError::VotingClosed),
            _ => return Err(GovernanceError::VotingClosed),
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
            id        = proposal_id,
            voter     = hex::encode(voter),
            power_wei = voter_stake,
            dir       = format!("{direction:?}"),
            "Governance: vote cast"
        );

        Ok(())
    }

    /// Finalize the voting period and transition the proposal state.
    ///
    /// * If the proposal passed   → `Queued { execute_after }`.
    /// * If it failed quorum/majority → `Defeated`.
    ///
    /// # Errors
    /// * [`GovernanceError::ProposalNotFound`]
    /// * [`GovernanceError::VotingStillActive`] — too early to finalize.
    /// * [`GovernanceError::AlreadyFinalized`] — not in `Active` state.
    pub fn finalize_voting(
        &mut self,
        proposal_id:   u64,
        current_block: u64,
    ) -> Result<ProposalState, GovernanceError> {
        let total = self.total_staked;
        let proposal = self.proposals.get_mut(&proposal_id)
            .ok_or(GovernanceError::ProposalNotFound(proposal_id))?;

        match proposal.state {
            ProposalState::Active { end_block } => {
                if current_block <= end_block {
                    return Err(GovernanceError::VotingStillActive {
                        remaining: end_block - current_block,
                    });
                }
            }
            _ => return Err(GovernanceError::AlreadyFinalized),
        }

        let new_state = if proposal.has_passed(total) {
            ProposalState::Queued {
                execute_after: current_block + TIMELOCK_DELAY_BLOCKS,
            }
        } else {
            ProposalState::Defeated
        };

        proposal.state = new_state.clone();

        tracing::info!(
            id    = proposal_id,
            state = format!("{new_state:?}"),
            "Governance: voting finalized"
        );

        Ok(new_state)
    }

    /// Execute a proposal that has cleared the timelock.
    ///
    /// Returns an [`ExecutionReceipt`] with the calldata to dispatch
    /// to the target contract.  The caller (admin service) is
    /// responsible for actually executing the on-chain call.
    ///
    /// # Errors
    /// * [`GovernanceError::ProposalNotFound`]
    /// * [`GovernanceError::TimelockNotExpired`]
    /// * [`GovernanceError::AlreadyExecuted`]
    /// * [`GovernanceError::Vetoed`]
    /// * [`GovernanceError::ProposalDefeated`]
    /// * [`GovernanceError::NotQueued`]
    pub fn execute_proposal(
        &mut self,
        proposal_id:   u64,
        current_block: u64,
    ) -> Result<ExecutionReceipt, GovernanceError> {
        let proposal = self.proposals.get_mut(&proposal_id)
            .ok_or(GovernanceError::ProposalNotFound(proposal_id))?;

        let execute_after = match &proposal.state {
            ProposalState::Queued { execute_after } => *execute_after,
            ProposalState::Executed { .. } => return Err(GovernanceError::AlreadyExecuted),
            ProposalState::Vetoed { reason } => return Err(GovernanceError::Vetoed(reason.clone())),
            ProposalState::Defeated          => return Err(GovernanceError::ProposalDefeated),
            ProposalState::Expired           => return Err(GovernanceError::ProposalExpired),
            _                                => return Err(GovernanceError::NotQueued),
        };

        if current_block < execute_after {
            return Err(GovernanceError::TimelockNotExpired {
                remaining: execute_after - current_block,
            });
        }

        let receipt = ExecutionReceipt {
            proposal_id,
            target:      proposal.target,
            calldata:    proposal.calldata.clone(),
            executed_at: current_block,
        };

        proposal.state = ProposalState::Executed { at_block: current_block };

        tracing::info!(
            id     = proposal_id,
            target = hex::encode(proposal.target),
            block  = current_block,
            "Governance: proposal executed"
        );

        Ok(receipt)
    }

    /// Guardian-only: veto a queued (or active) proposal before execution.
    ///
    /// # Errors
    /// * [`GovernanceError::NotGuardian`] — `sender` is not the guardian.
    /// * [`GovernanceError::ProposalNotFound`]
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

        tracing::warn!(
            id     = proposal_id,
            reason = %reason,
            "Governance: guardian veto"
        );

        proposal.state = ProposalState::Vetoed { reason };
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Receipt
// ---------------------------------------------------------------------------

/// Returned after a successful proposal execution.
///
/// The admin service must dispatch `calldata` to `target`
/// (e.g. via the system precompile proxy) after receiving this.
#[derive(Debug)]
pub struct ExecutionReceipt {
    /// ID of the executed proposal.
    pub proposal_id:  u64,
    /// Target contract address.
    pub target:       [u8; 20],
    /// ABI-encoded calldata to execute on `target`.
    pub calldata:     Vec<u8>,
    /// Block at which execution was recorded.
    pub executed_at:  u64,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

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

    #[error("proposal already finalized (not Active)")]
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

    #[error("caller is not the guardian multisig")]
    NotGuardian,

    #[error("insufficient stake to propose: have {have} wei, need {need} wei")]
    InsufficientStakeToPropose { have: u128, need: u128 },

    #[error("invalid proposal title (must be 1–256 chars)")]
    InvalidTitle,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn guardian() -> [u8; 20] { [0xFF; 20] }
    fn proposer() -> [u8; 20] { [0x01; 20] }
    fn voter1()   -> [u8; 20] { [0x10; 20] }
    fn voter2()   -> [u8; 20] { [0x20; 20] }

    const STAKE_1M: u128 = 1_000_000 * 1_000_000_000_000_000_000u128;

    fn gov() -> Governance {
        Governance::new(guardian(), STAKE_1M * 10) // 10M ZBX total staked
    }

    fn make_proposal(gov: &mut Governance, block: u64) -> u64 {
        gov.create_proposal(
            proposer(),
            STAKE_1M, // 1M ZBX ≫ 1 000 ZBX minimum
            "ZEP-013: Increase block gas limit".into(),
            "Proposal to increase MAX_BLOCK_GAS from 30M to 50M.".into(),
            [0xC4; 20],
            vec![0x01, 0x02, 0x03],
            block,
        )
        .unwrap()
    }

    #[test]
    fn create_proposal_success() {
        let mut gov = gov();
        let id = make_proposal(&mut gov, 100);
        assert_eq!(id, 1);
        assert!(gov.proposals.contains_key(&1));
    }

    #[test]
    fn propose_with_low_stake_rejected() {
        let mut gov = gov();
        let tiny = 100 * 1_000_000_000_000_000_000u128; // 100 ZBX < 1 000 ZBX min
        let err = gov.create_proposal(
            proposer(), tiny,
            "Bad proposal".into(), "".into(),
            [0x00; 20], vec![], 100,
        )
        .unwrap_err();
        assert!(matches!(err, GovernanceError::InsufficientStakeToPropose { .. }));
    }

    #[test]
    fn empty_title_rejected() {
        let mut gov = gov();
        let err = gov.create_proposal(
            proposer(), STAKE_1M,
            "".into(), "".into(),
            [0x00; 20], vec![], 0,
        )
        .unwrap_err();
        assert!(matches!(err, GovernanceError::InvalidTitle));
    }

    #[test]
    fn full_proposal_lifecycle() {
        let mut gov = gov();

        let id = make_proposal(&mut gov, 0);

        // voter1: 2M ZBX For; voter2: 500k ZBX Against.
        // Total staked = 10M. Quorum = 1M → 2M ≥ 1M ✓. Majority 2M > 500k ✓.
        gov.cast_vote(voter1(), STAKE_1M * 2, id, VoteDirection::For,     1).unwrap();
        gov.cast_vote(voter2(), STAKE_1M / 2, id, VoteDirection::Against, 1).unwrap();

        let state = gov.finalize_voting(id, VOTING_PERIOD_BLOCKS + 1).unwrap();
        assert!(matches!(state, ProposalState::Queued { .. }));

        let receipt = gov
            .execute_proposal(id, VOTING_PERIOD_BLOCKS + TIMELOCK_DELAY_BLOCKS + 2)
            .unwrap();
        assert_eq!(receipt.proposal_id, id);
        assert_eq!(receipt.target, [0xC4; 20]);
        assert_eq!(receipt.calldata, vec![0x01, 0x02, 0x03]);
    }

    #[test]
    fn low_vote_count_leads_to_defeat() {
        let mut gov = gov();
        let id = make_proposal(&mut gov, 0);
        // voter with only 0.5% of total staked — below 10% quorum
        gov.cast_vote(voter1(), STAKE_1M / 20, id, VoteDirection::For, 1).unwrap();
        let state = gov.finalize_voting(id, VOTING_PERIOD_BLOCKS + 1).unwrap();
        assert_eq!(state, ProposalState::Defeated);
    }

    #[test]
    fn double_vote_rejected() {
        let mut gov = gov();
        let id = make_proposal(&mut gov, 0);
        gov.cast_vote(voter1(), STAKE_1M, id, VoteDirection::For, 1).unwrap();
        let err = gov.cast_vote(voter1(), STAKE_1M, id, VoteDirection::For, 2).unwrap_err();
        assert!(matches!(err, GovernanceError::AlreadyVoted));
    }

    #[test]
    fn finalize_too_early_rejected() {
        let mut gov = gov();
        let id = make_proposal(&mut gov, 0);
        gov.cast_vote(voter1(), STAKE_1M * 5, id, VoteDirection::For, 1).unwrap();
        // Voting period ends at block VOTING_PERIOD_BLOCKS, so block 1 is too early.
        let err = gov.finalize_voting(id, 1).unwrap_err();
        assert!(matches!(err, GovernanceError::VotingStillActive { .. }));
    }

    #[test]
    fn execute_before_timelock_rejected() {
        let mut gov = gov();
        let id = make_proposal(&mut gov, 0);
        gov.cast_vote(voter1(), STAKE_1M * 5, id, VoteDirection::For, 1).unwrap();
        gov.finalize_voting(id, VOTING_PERIOD_BLOCKS + 1).unwrap();
        // Timelock expires at VOTING_PERIOD_BLOCKS + 1 + TIMELOCK_DELAY_BLOCKS,
        // so trying one block earlier must fail.
        let err = gov
            .execute_proposal(id, VOTING_PERIOD_BLOCKS + TIMELOCK_DELAY_BLOCKS)
            .unwrap_err();
        assert!(matches!(err, GovernanceError::TimelockNotExpired { .. }));
    }

    #[test]
    fn guardian_can_veto() {
        let mut gov = gov();
        let id = make_proposal(&mut gov, 0);
        gov.cast_vote(voter1(), STAKE_1M * 5, id, VoteDirection::For, 1).unwrap();
        gov.finalize_voting(id, VOTING_PERIOD_BLOCKS + 1).unwrap();

        gov.veto_proposal(guardian(), id, "Security concern".into()).unwrap();

        let err = gov
            .execute_proposal(id, VOTING_PERIOD_BLOCKS + TIMELOCK_DELAY_BLOCKS + 2)
            .unwrap_err();
        assert!(matches!(err, GovernanceError::Vetoed(_)));
    }

    #[test]
    fn non_guardian_cannot_veto() {
        let mut gov = gov();
        let id = make_proposal(&mut gov, 0);
        let err = gov.veto_proposal([0x99; 20], id, "".into()).unwrap_err();
        assert!(matches!(err, GovernanceError::NotGuardian));
    }

    #[test]
    fn voting_period_is_7_days() {
        // 7 days × 86 400 s/day ÷ 2 s/block = 302 400
        assert_eq!(VOTING_PERIOD_BLOCKS, 302_400);
    }

    #[test]
    fn timelock_is_2_days() {
        // 2 days × 86 400 ÷ 2 = 86 400
        assert_eq!(TIMELOCK_DELAY_BLOCKS, 86_400);
    }

    #[test]
    fn quorum_bps_is_10_percent() {
        assert_eq!(QUORUM_BPS, 1_000);
    }

    #[test]
    fn update_total_staked_affects_quorum() {
        let mut gov = gov(); // 10M staked
        let id = make_proposal(&mut gov, 0);
        // 1M vote For — passes quorum at 10M total (10%)
        gov.cast_vote(voter1(), STAKE_1M, id, VoteDirection::For, 1).unwrap();

        // Inflate total staked to 100M — same votes now < quorum
        gov.update_total_staked(STAKE_1M * 100);

        let state = gov.finalize_voting(id, VOTING_PERIOD_BLOCKS + 1).unwrap();
        assert_eq!(state, ProposalState::Defeated);
    }
}
