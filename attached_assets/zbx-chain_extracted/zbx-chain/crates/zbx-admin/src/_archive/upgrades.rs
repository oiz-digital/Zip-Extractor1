//! Protocol upgrade administration -- hard fork scheduling + upgrade votes.
//!
//! ZBX protocol upgrades (ZEPs) follow this lifecycle:
//!   1. ZEP proposal submitted on-chain
//!   2. Governance vote (validator-weighted)
//!   3. If vote passes: Upgrader role schedules hard fork
//!   4. 48h timelock before activation
//!   5. At activation_block: new rules take effect
//!
//! Hard fork list (planned):
//!   ZEP-001: Genesis (block 0)
//!   ZEP-002: EVM 2.0 opcodes (block 500000)
//!   ZEP-003: ZK proof integration (block 1000000)
//!   ZEP-004: Sharding phase 1 (block 2000000)

// ── Hard Fork Registry ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct HardFork {
    pub id:               u32,
    pub name:             String,          // e.g. "ZEP-002"
    pub description:      String,
    pub activation_block: u64,            // block at which new rules activate
    pub scheduled_at:     u64,            // block when this was scheduled
    pub scheduled_by:     [u8; 20],       // Upgrader who scheduled it
    pub proposal_id:      Option<u64>,    // linked governance proposal
    pub status:           ForkStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForkStatus {
    Pending,    // scheduled, awaiting activation_block
    Active,     // activation_block reached, rules in effect
    Cancelled,  // cancelled before activation (SuperAdmin only)
}

/// Schedule a hard fork activation.
/// Upgrader role required. Emits AdminEvent::HardForkScheduled.
/// Timelock: activation_block must be >= current_block + MIN_FORK_DELAY.
pub const MIN_FORK_DELAY_BLOCKS: u64 = 57_600; // ~48h at 3s/block

pub struct ForkScheduler {
    pub forks:    Vec<HardFork>,
    pub next_id:  u32,
}

impl ForkScheduler {
    pub fn new() -> Self { Self { forks: Vec::new(), next_id: 1 } }

    /// Schedule a hard fork.
    /// activation_block must be at least MIN_FORK_DELAY_BLOCKS in the future.
    pub fn schedule_hard_fork(
        &mut self,
        name:             String,
        description:      String,
        activation_block: u64,
        current_block:    u64,
        scheduled_by:     [u8; 20],
        proposal_id:      Option<u64>,
    ) -> Result<HardFork, UpgradeError> {
        if activation_block < current_block + MIN_FORK_DELAY_BLOCKS {
            return Err(UpgradeError::TimelockNotExpired {
                required: current_block + MIN_FORK_DELAY_BLOCKS,
                given:    activation_block,
            });
        }
        // Check no other fork scheduled at same block
        if self.forks.iter().any(|f| f.activation_block == activation_block && f.status == ForkStatus::Pending) {
            return Err(UpgradeError::BlockConflict(activation_block));
        }
        let fork = HardFork {
            id: self.next_id, name, description, activation_block,
            scheduled_at: current_block, scheduled_by, proposal_id,
            status: ForkStatus::Pending,
        };
        self.next_id += 1;
        self.forks.push(fork.clone());
        Ok(fork)
    }

    /// Cancel a pending hard fork (SuperAdmin only).
    pub fn cancel_fork(&mut self, fork_id: u32) -> Result<(), UpgradeError> {
        let fork = self.forks.iter_mut()
            .find(|f| f.id == fork_id)
            .ok_or(UpgradeError::ForkNotFound(fork_id))?;
        if fork.status != ForkStatus::Pending {
            return Err(UpgradeError::NotPending);
        }
        fork.status = ForkStatus::Cancelled;
        Ok(())
    }

    /// Check if a hard fork is active at the given block.
    pub fn is_fork_active(&self, fork_name: &str, block: u64) -> bool {
        self.forks.iter().any(|f| f.name == fork_name && f.activation_block <= block && f.status != ForkStatus::Cancelled)
    }

    /// Get the next pending fork.
    pub fn next_pending(&self) -> Option<&HardFork> {
        self.forks.iter()
            .filter(|f| f.status == ForkStatus::Pending)
            .min_by_key(|f| f.activation_block)
    }
}

// ── Protocol Upgrade Vote ─────────────────────────────────────────────────────

/// On-chain governance vote for a protocol upgrade (ZEP).
///
/// Vote mechanics:
///   - Each validator votes with their staked weight
///   - Quorum required: 67% of total stake must participate
///   - Passing threshold: >50% of participating stake votes YES
///   - Voting period: 7 days (201,600 blocks at 3s)
///
/// Emits AdminEvent::UpgradeVotePassed when quorum + majority reached.
pub const VOTING_PERIOD_BLOCKS: u64 = 201_600; // 7 days
pub const QUORUM_PCT: u64 = 67;               // 67% participation
pub const PASS_THRESHOLD_PCT: u64 = 51;       // 51% yes votes to pass

#[derive(Debug, Clone)]
pub struct UpgradeVote {
    pub proposal_id:    u64,
    pub fork_name:      String,
    pub start_block:    u64,
    pub end_block:      u64,
    pub yes_votes:      u128,  // stake-weighted YES
    pub no_votes:       u128,  // stake-weighted NO
    pub abstain:        u128,
    pub total_stake:    u128,
    pub votes_cast:     Vec<VoteCast>,
    pub status:         VoteStatus,
}

#[derive(Debug, Clone)]
pub struct VoteCast {
    pub validator: [u8; 20],
    pub stake:     u128,
    pub vote:      Vote,
    pub block:     u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Vote { Yes, No, Abstain }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoteStatus { Active, Passed, Failed, Expired }

impl UpgradeVote {
    pub fn new(proposal_id: u64, fork_name: String, start_block: u64, total_stake: u128) -> Self {
        Self {
            proposal_id, fork_name, start_block,
            end_block: start_block + VOTING_PERIOD_BLOCKS,
            yes_votes: 0, no_votes: 0, abstain: 0, total_stake,
            votes_cast: Vec::new(), status: VoteStatus::Active,
        }
    }

    /// Cast a protocol upgrade vote (validator-weighted).
    pub fn cast_vote(&mut self, validator: [u8; 20], stake: u128, vote: Vote, block: u64) -> Result<(), UpgradeError> {
        if block > self.end_block { return Err(UpgradeError::VotingPeriodExpired); }
        if self.votes_cast.iter().any(|v| v.validator == validator) {
            return Err(UpgradeError::AlreadyVoted);
        }
        match &vote {
            Vote::Yes     => self.yes_votes += stake,
            Vote::No      => self.no_votes  += stake,
            Vote::Abstain => self.abstain   += stake,
        }
        self.votes_cast.push(VoteCast { validator, stake, vote, block });
        self.update_status();
        Ok(())
    }

    /// Check quorum + passing threshold. Emits UpgradeVotePassed if approved.
    fn update_status(&mut self) {
        let participated = self.yes_votes + self.no_votes + self.abstain;
        let quorum_reached = participated * 100 / self.total_stake.max(1) >= QUORUM_PCT;
        if quorum_reached {
            let yes_pct = self.yes_votes * 100 / participated.max(1);
            if yes_pct >= PASS_THRESHOLD_PCT {
                self.status = VoteStatus::Passed;
            }
        }
    }

    /// Whether the upgrade vote passed.
    pub fn is_passed(&self) -> bool { self.status == VoteStatus::Passed }
}

#[derive(Debug)]
pub enum UpgradeError {
    TimelockNotExpired { required: u64, given: u64 },
    BlockConflict(u64),
    ForkNotFound(u32),
    NotPending,
    VotingPeriodExpired,
    AlreadyVoted,
    InsufficientQuorum,
}