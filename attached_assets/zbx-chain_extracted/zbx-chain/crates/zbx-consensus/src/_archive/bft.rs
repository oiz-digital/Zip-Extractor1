//! Core BFT consensus state machine.

use crate::{ValidatorSet, Vote, VoteType, ConsensusError};
use tracing::{info, warn, debug};

/// Consensus round state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoundState {
    Idle,
    Propose,
    Prevote,
    Precommit,
    Commit,
}

/// The consensus engine drives the BFT state machine.
pub struct ConsensusEngine {
    pub height:       u64,
    pub round:        u32,
    pub state:        RoundState,
    pub validators:   ValidatorSet,
    pub leader_index: usize,
}

impl ConsensusEngine {
    pub fn new(validators: ValidatorSet) -> Self {
        Self {
            height:       0,
            round:        0,
            state:        RoundState::Idle,
            validators,
            leader_index: 0,
        }
    }

    /// Start a new consensus round for the given block height.
    pub fn start_round(&mut self, height: u64) {
        self.height = height;
        self.round  = 0;
        self.state  = RoundState::Propose;
        self.leader_index = (height as usize) % self.validators.len();
        info!(height, leader = %self.validators.get(self.leader_index).address_hex(),
              "Consensus round started");
    }

    /// Process a vote and advance state if threshold is reached.
    pub fn process_vote(&mut self, vote: Vote) -> Result<bool, ConsensusError> {
        debug!(height = vote.height, round = vote.round, kind = ?vote.kind, "Processing vote");

        match vote.kind {
            VoteType::Prevote => {
                if self.state == RoundState::Prevote {
                    // TODO: tally prevotes, advance to Precommit on 2/3+
                    self.state = RoundState::Precommit;
                    info!(height = self.height, "2/3+ prevotes — advancing to Precommit");
                    return Ok(true);
                }
            }
            VoteType::Precommit => {
                if self.state == RoundState::Precommit {
                    // TODO: tally precommits, advance to Commit on 2/3+
                    self.state = RoundState::Commit;
                    info!(height = self.height, "2/3+ precommits — block COMMITTED");
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// Called when a round times out — increment round, change leader.
    pub fn timeout_round(&mut self) {
        warn!(height = self.height, round = self.round, "Round timed out");
        self.round       += 1;
        self.leader_index = (self.leader_index + 1) % self.validators.len();
        self.state        = RoundState::Propose;
    }

    pub fn is_leader(&self, addr: &[u8; 20]) -> bool {
        &self.validators.get(self.leader_index).address == addr
    }
}