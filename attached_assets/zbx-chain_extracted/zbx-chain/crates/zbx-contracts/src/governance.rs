//! ZEP governance contract — on-chain proposal voting.

use std::collections::HashMap;
use zbx_types::address::Address;

pub type ProposalId = u64;

/// Governance proposal states.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProposalState {
    Pending,
    Active { deadline: u64 },
    Succeeded,
    Defeated,
    Executed,
    Cancelled,
}

/// An on-chain governance proposal.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Proposal {
    pub id:          ProposalId,
    pub proposer:    Address,
    pub description: String,
    pub state:       ProposalState,
    pub votes_for:   u128,
    pub votes_against: u128,
    pub votes_abstain: u128,
}

/// Governance contract state.
#[derive(Debug, Default)]
pub struct GovernanceContract {
    proposals:    HashMap<ProposalId, Proposal>,
    next_id:      ProposalId,
    votes_cast:   HashMap<(ProposalId, Address), bool>,
    /// Minimum ZBX votes needed to reach quorum (1M ZBX with 18 decimals).
    pub quorum:   u128,
    /// Voting period in seconds (3 days).
    pub voting_period: u64,
    /// Minimum ZBX stake a proposer must hold to create a proposal.
    /// Prevents spam from zero-balance addresses (S-LOW-03).
    /// Default: 100 ZBX (100 * 10^18).
    pub min_proposal_stake: u128,
}

impl GovernanceContract {
    pub fn new() -> Self {
        Self {
            quorum: 1_000_000 * 1_000_000_000_000_000_000,
            voting_period: 3 * 24 * 3600,
            min_proposal_stake: 100 * 1_000_000_000_000_000_000, // 100 ZBX
            ..Default::default()
        }
    }

    /// Submit a new governance proposal.
    ///
    /// # Errors
    /// Returns `Err("insufficient stake")` if `proposer_stake < min_proposal_stake` (S-LOW-03).
    pub fn propose(
        &mut self,
        proposer: Address,
        description: String,
        proposer_stake: u128,
        now: u64,
    ) -> Result<ProposalId, &'static str> {
        if proposer_stake < self.min_proposal_stake {
            return Err("insufficient stake to propose");
        }
        let id = self.next_id;
        self.next_id += 1;
        self.proposals.insert(id, Proposal {
            id, proposer, description,
            state: ProposalState::Active { deadline: now + self.voting_period },
            votes_for: 0, votes_against: 0, votes_abstain: 0,
        });
        Ok(id)
    }

    pub fn vote(&mut self, proposal_id: ProposalId, voter: Address, support: i8, weight: u128, now: u64) -> Result<(), &'static str> {
        let prop = self.proposals.get_mut(&proposal_id).ok_or("unknown proposal")?;
        if let ProposalState::Active { deadline } = prop.state {
            if now > deadline { return Err("voting closed"); }
        } else {
            return Err("not active");
        }
        if self.votes_cast.contains_key(&(proposal_id, voter)) { return Err("already voted"); }
        self.votes_cast.insert((proposal_id, voter), true);
        match support {
            1  => prop.votes_for     += weight,
            -1 => prop.votes_against += weight,
            _  => prop.votes_abstain += weight,
        }
        Ok(())
    }

    pub fn finalise(&mut self, proposal_id: ProposalId, now: u64) -> Result<(), &'static str> {
        let prop = self.proposals.get_mut(&proposal_id).ok_or("unknown")?;
        if let ProposalState::Active { deadline } = prop.state {
            if now <= deadline { return Err("voting still open"); }
            let total = prop.votes_for + prop.votes_against + prop.votes_abstain;
            prop.state = if total >= self.quorum && prop.votes_for > prop.votes_against {
                ProposalState::Succeeded
            } else {
                ProposalState::Defeated
            };
            Ok(())
        } else {
            Err("not active")
        }
    }
}
