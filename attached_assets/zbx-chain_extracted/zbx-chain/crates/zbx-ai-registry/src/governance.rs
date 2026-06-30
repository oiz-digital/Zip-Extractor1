//! Model Governance — on-chain proposal, voting, and activation.
//!
//! All model lifecycle changes (submit, activate, deprecate, suspend) require
//! a governance vote. Voting follows ZBX DAO rules:
//!
//! - Quorum:       >= 3 approvers required
//! - Threshold:    >= 66% YES to pass (2/3 supermajority)
//! - Time limit:   Proposal expires after VOTE_TIMEOUT_BLOCKS blocks
//! - Veto:         Any Core Guardian can veto (emergency security)
//!
//! Security:
//! - Each address can only vote once per proposal
//! - Expired proposals are automatically rejected
//! - Veto is permanent and cannot be overridden by further votes
//! - All votes are logged with block number for auditability

use crate::{
    registry::{ModelEntry, ModelRegistry, ModelStatus},
    error::RegistryError,
};
use zbx_ai_precompile::ModelId;
use serde::{Serialize, Deserialize};
use std::collections::HashMap;

/// Voting timeout in blocks (~7 days at 2s block time).
pub const VOTE_TIMEOUT_BLOCKS: u64 = 302_400;

/// Minimum approvers for quorum.
pub const QUORUM: usize = 3;

/// Supermajority threshold in basis points (66%).
pub const PASS_THRESHOLD_BPS: u32 = 6_600;

/// Governance action type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GovernanceAction {
    /// Activate a pending model.
    Activate { model_id: ModelId },
    /// Deprecate an active model.
    Deprecate { model_id: ModelId },
    /// Suspend a model due to security issue.
    Suspend { model_id: ModelId, reason: String },
    /// Update fee schedule for a model.
    UpdateFee { model_id: ModelId, new_fee_wei: u128 },
    /// Add a new core guardian.
    AddGuardian { address: [u8; 20] },
    /// Remove a core guardian.
    RemoveGuardian { address: [u8; 20] },
}

/// Proposal state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProposalStatus {
    Active,
    Passed,
    Rejected,
    Expired,
    Vetoed { by: [u8; 20] },
    Executed,
}

/// A governance vote cast by one address.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vote {
    pub voter:       [u8; 20],
    pub approve:     bool,
    pub block_number: u64,
    pub comment:     String,
}

/// A governance proposal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proposal {
    /// Unique proposal ID.
    pub id:           u64,
    /// Action being proposed.
    pub action:       GovernanceAction,
    /// Proposer address.
    pub proposer:     [u8; 20],
    /// Block when proposal was submitted.
    pub submitted_at: u64,
    /// Block when proposal expires.
    pub expires_at:   u64,
    /// All votes cast.
    pub votes:        Vec<Vote>,
    /// Current status.
    pub status:       ProposalStatus,
    /// Optional description.
    pub description:  String,
}

impl Proposal {
    pub fn new(
        id:          u64,
        action:      GovernanceAction,
        proposer:    [u8; 20],
        block:       u64,
        description: String,
    ) -> Self {
        Self {
            id,
            action,
            proposer,
            submitted_at: block,
            expires_at:   block + VOTE_TIMEOUT_BLOCKS,
            votes:        vec![],
            status:       ProposalStatus::Active,
            description,
        }
    }

    pub fn is_expired(&self, current_block: u64) -> bool {
        current_block > self.expires_at
    }

    pub fn vote_count(&self) -> (usize, usize) {
        let yes = self.votes.iter().filter(|v| v.approve).count();
        let no  = self.votes.iter().filter(|v| !v.approve).count();
        (yes, no)
    }

    pub fn has_voted(&self, addr: &[u8; 20]) -> bool {
        self.votes.iter().any(|v| &v.voter == addr)
    }

    pub fn has_quorum(&self) -> bool {
        let (yes, _) = self.vote_count();
        yes >= QUORUM
    }

    pub fn passes(&self) -> bool {
        let (yes, no) = self.vote_count();
        let total = yes + no;
        if total == 0 { return false; }
        (yes as u32 * 10_000 / total as u32) >= PASS_THRESHOLD_BPS
    }
}

/// The governance system — manages proposals, voting, and execution.
pub struct GovernanceSystem {
    proposals:  Vec<Proposal>,
    next_id:    u64,
    /// Core guardians (can veto any proposal).
    guardians:  Vec<[u8; 20]>,
    /// All registered governance participants.
    participants: Vec<[u8; 20]>,
}

impl GovernanceSystem {
    pub fn new(initial_guardians: Vec<[u8; 20]>) -> Self {
        let participants = initial_guardians.clone();
        Self {
            proposals:    vec![],
            next_id:      1,
            guardians:    initial_guardians,
            participants,
        }
    }

    /// Submit a new proposal.
    pub fn propose(
        &mut self,
        action:      GovernanceAction,
        proposer:    [u8; 20],
        block:       u64,
        description: String,
    ) -> Result<u64, RegistryError> {
        if !self.is_participant(&proposer) {
            return Err(RegistryError::NotAuthorized {
                addr:   hex_addr(&proposer),
                action: "propose".to_string(),
            });
        }
        let id = self.next_id;
        self.next_id += 1;
        self.proposals.push(Proposal::new(id, action, proposer, block, description));
        tracing::info!(proposal_id = id, block, "Governance proposal submitted");
        Ok(id)
    }

    /// Cast a vote on a proposal.
    pub fn vote(
        &mut self,
        proposal_id: u64,
        voter:       [u8; 20],
        approve:     bool,
        block:       u64,
        comment:     String,
    ) -> Result<(), RegistryError> {
        let proposal = self.get_proposal_mut(proposal_id)?;

        if proposal.status != ProposalStatus::Active {
            return Err(RegistryError::ProposalNotActive(proposal_id));
        }
        if proposal.is_expired(block) {
            proposal.status = ProposalStatus::Expired;
            return Err(RegistryError::ProposalExpired(proposal_id));
        }
        if proposal.has_voted(&voter) {
            return Err(RegistryError::AlreadyVoted { voter: hex_addr(&voter) });
        }

        proposal.votes.push(Vote { voter, approve, block_number: block, comment });

        tracing::info!(
            proposal_id, approve, voter = ?voter, block,
            "Governance vote cast"
        );
        Ok(())
    }

    /// Guardian veto — immediately kills a proposal.
    pub fn veto(
        &mut self,
        proposal_id: u64,
        guardian:    [u8; 20],
        block:       u64,
    ) -> Result<(), RegistryError> {
        if !self.is_guardian(&guardian) {
            return Err(RegistryError::NotAuthorized {
                addr:   hex_addr(&guardian),
                action: "veto".to_string(),
            });
        }
        let proposal = self.get_proposal_mut(proposal_id)?;
        proposal.status = ProposalStatus::Vetoed { by: guardian };
        tracing::warn!(
            proposal_id, guardian = ?guardian, block,
            "Governance proposal VETOED by guardian"
        );
        Ok(())
    }

    /// Try to finalize and execute a proposal.
    pub fn try_execute(
        &mut self,
        proposal_id: u64,
        registry:    &mut ModelRegistry,
        block:       u64,
    ) -> Result<bool, RegistryError> {
        let proposal = self.get_proposal_mut(proposal_id)?;

        if proposal.status != ProposalStatus::Active {
            return Ok(false);
        }
        if proposal.is_expired(block) {
            proposal.status = ProposalStatus::Expired;
            return Ok(false);
        }
        if !proposal.has_quorum() || !proposal.passes() {
            return Ok(false);
        }

        proposal.status = ProposalStatus::Passed;
        let action = proposal.action.clone();

        // Execute the governance action
        match action {
            GovernanceAction::Activate { model_id } => {
                if let Some(entry) = registry.get_latest_mut(model_id) {
                    entry.activate(block)?;
                    tracing::info!(?model_id, block, "Model activated via governance");
                }
            }
            GovernanceAction::Deprecate { model_id } => {
                if let Some(entry) = registry.get_latest_mut(model_id) {
                    entry.deprecate(block)?;
                    tracing::info!(?model_id, block, "Model deprecated via governance");
                }
            }
            GovernanceAction::Suspend { model_id, reason } => {
                if let Some(entry) = registry.get_latest_mut(model_id) {
                    entry.suspend(reason.clone());
                    tracing::warn!(?model_id, reason, block, "Model SUSPENDED via governance");
                }
            }
            GovernanceAction::AddGuardian { address } => {
                if !self.guardians.contains(&address) {
                    self.guardians.push(address);
                    self.participants.push(address);
                }
            }
            GovernanceAction::RemoveGuardian { address } => {
                self.guardians.retain(|g| g != &address);
            }
            GovernanceAction::UpdateFee { .. } => {
                // Fee updates handled by BillingSystem separately
            }
        }

        let proposal = self.get_proposal_mut(proposal_id)?;
        proposal.status = ProposalStatus::Executed;
        Ok(true)
    }

    pub fn get_proposal(&self, id: u64) -> Option<&Proposal> {
        self.proposals.iter().find(|p| p.id == id)
    }

    fn get_proposal_mut(&mut self, id: u64) -> Result<&mut Proposal, RegistryError> {
        self.proposals.iter_mut().find(|p| p.id == id)
            .ok_or(RegistryError::ProposalNotFound(id))
    }

    pub fn is_guardian(&self, addr: &[u8; 20]) -> bool {
        self.guardians.contains(addr)
    }

    pub fn is_participant(&self, addr: &[u8; 20]) -> bool {
        self.participants.contains(addr)
    }

    pub fn add_participant(&mut self, addr: [u8; 20]) {
        if !self.participants.contains(&addr) {
            self.participants.push(addr);
        }
    }

    pub fn guardian_count(&self) -> usize { self.guardians.len() }
    pub fn proposal_count(&self) -> usize { self.proposals.len() }
}

fn hex_addr(a: &[u8; 20]) -> String {
    a.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{ModelRegistry, ModelEntry, ModelTier};

    fn guardians() -> Vec<[u8; 20]> {
        vec![[0x01u8; 20], [0x02u8; 20], [0x03u8; 20]]
    }

    fn entry() -> ModelEntry {
        ModelEntry::new(
            ModelId::SpamClassifier, "1.0.0".to_string(), "spam-v1".to_string(),
            [0x01u8; 20], [0u8; 32], 512, ModelTier::Community, "desc".to_string(), 1,
        ).unwrap()
    }

    #[test]
    fn proposal_and_pass() {
        let mut gov = GovernanceSystem::new(guardians());
        let proposer = [0x01u8; 20];
        let id = gov.propose(
            GovernanceAction::Activate { model_id: ModelId::SpamClassifier },
            proposer, 1000, "Activate spam classifier".to_string(),
        ).unwrap();

        // 3 YES votes → quorum + supermajority
        gov.vote(id, [0x01u8; 20], true, 1001, "".to_string()).unwrap();
        gov.vote(id, [0x02u8; 20], true, 1002, "".to_string()).unwrap();
        gov.vote(id, [0x03u8; 20], true, 1003, "".to_string()).unwrap();

        let mut reg = ModelRegistry::new();
        reg.submit(entry()).unwrap();

        let executed = gov.try_execute(id, &mut reg, 1010).unwrap();
        assert!(executed);
        assert!(reg.get_active(ModelId::SpamClassifier).is_some());
    }

    #[test]
    fn veto_blocks_execution() {
        let mut gov = GovernanceSystem::new(guardians());
        let id = gov.propose(
            GovernanceAction::Activate { model_id: ModelId::RiskScorer },
            [0x01u8; 20], 1000, "".to_string(),
        ).unwrap();
        gov.vote(id, [0x01u8; 20], true, 1001, "".to_string()).unwrap();
        gov.vote(id, [0x02u8; 20], true, 1002, "".to_string()).unwrap();
        gov.vote(id, [0x03u8; 20], true, 1003, "".to_string()).unwrap();
        gov.veto(id, [0x01u8; 20], 1004).unwrap();

        let mut reg = ModelRegistry::new();
        let executed = gov.try_execute(id, &mut reg, 1010).unwrap();
        assert!(!executed);
        assert!(matches!(gov.get_proposal(id).unwrap().status, ProposalStatus::Vetoed { .. }));
    }

    #[test]
    fn double_vote_rejected() {
        let mut gov = GovernanceSystem::new(guardians());
        let id = gov.propose(
            GovernanceAction::Activate { model_id: ModelId::NftTagger },
            [0x01u8; 20], 1000, "".to_string(),
        ).unwrap();
        gov.vote(id, [0x01u8; 20], true, 1001, "".to_string()).unwrap();
        let err = gov.vote(id, [0x01u8; 20], true, 1002, "".to_string()).unwrap_err();
        assert!(matches!(err, RegistryError::AlreadyVoted { .. }));
    }

    #[test]
    fn non_participant_cannot_propose() {
        let mut gov = GovernanceSystem::new(guardians());
        let outsider = [0xFFu8; 20];
        let err = gov.propose(
            GovernanceAction::Activate { model_id: ModelId::SpamClassifier },
            outsider, 1, "".to_string(),
        ).unwrap_err();
        assert!(matches!(err, RegistryError::NotAuthorized { .. }));
    }

    #[test]
    fn insufficient_votes_not_executed() {
        let mut gov = GovernanceSystem::new(guardians());
        let id = gov.propose(
            GovernanceAction::Activate { model_id: ModelId::SpamClassifier },
            [0x01u8; 20], 1000, "".to_string(),
        ).unwrap();
        // Only 2 votes — below quorum
        gov.vote(id, [0x01u8; 20], true, 1001, "".to_string()).unwrap();
        gov.vote(id, [0x02u8; 20], true, 1002, "".to_string()).unwrap();
        let mut reg = ModelRegistry::new();
        let executed = gov.try_execute(id, &mut reg, 1010).unwrap();
        assert!(!executed);
    }
}
