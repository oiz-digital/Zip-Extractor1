//! RoundManager: orchestrates proposals, votes, and timeouts.

use crate::{error::ConsensusError, hotstuff::{ConsensusEvent, HotStuffConsensus}};
use zbx_types::{address::Address, block::Block};
use tracing::info;

pub use crate::liveness::RoundState;

/// High-level coordinator between consensus events and network I/O.
pub struct RoundManager {
    consensus: HotStuffConsensus,
}

impl RoundManager {
    pub fn new(consensus: HotStuffConsensus) -> Self {
        RoundManager { consensus }
    }

    pub fn current_round(&self) -> u64 {
        self.consensus.current_round()
    }

    pub fn committed_height(&self) -> u64 {
        self.consensus.committed_height
    }

    /// True if this node is the designated proposer for the current round.
    pub fn is_proposer(&self) -> bool {
        let round = self.current_round();
        self.consensus.validator_set.proposer_for_round(round) == self.consensus.my_address
    }

    /// Process an inbound proposal block from the network.
    pub fn process_proposal(
        &mut self,
        block: Block,
        parent_qc: crate::vote::QuorumCertificate,
    ) -> Result<Vec<ConsensusEvent>, ConsensusError> {
        info!(
            round = block.number(),
            hash = hex::encode(&block.hash()[..8]),
            "processing proposal"
        );
        self.consensus.on_proposal(block, parent_qc)
    }

    /// Process an inbound vote from a peer validator.
    pub fn process_vote(
        &mut self,
        vote: crate::vote::Vote,
        pubkey: zbx_crypto::bls::BlsPubKey,
    ) -> Result<Vec<ConsensusEvent>, ConsensusError> {
        self.consensus.on_vote(vote, pubkey)
    }

    /// Called when the pacemaker timer fires.
    pub fn process_timeout(&mut self) -> ConsensusEvent {
        self.consensus.on_timeout()
    }
}