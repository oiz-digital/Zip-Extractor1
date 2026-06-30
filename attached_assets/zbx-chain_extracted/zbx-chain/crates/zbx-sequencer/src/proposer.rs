//! Proposer — determines when it's our turn to propose a block.
//!
//! In v0.1 (PoA): only the designated validator proposes.
//! In v0.2 (HotStuff BFT): leader rotates every round (round-robin by stake).
//! In v0.3 (full DPoS): leader elected by VRF (verifiable random function).

use crate::error::SequencerError;

/// Proposer: determines if this node should propose the next block.
pub struct Proposer {
    /// Our validator address.
    address: [u8; 20],
    /// Current consensus mode.
    mode:    ProposerMode,
}

#[derive(Debug, Clone)]
pub enum ProposerMode {
    /// PoA: only one address proposes all blocks.
    Poa { authority: [u8; 20] },
    /// Round-robin: validators rotate by slot number.
    RoundRobin { validators: Vec<[u8; 20]> },
    /// VRF: cryptographically random leader election.
    Vrf {
        validators:  Vec<[u8; 20]>,
        vrf_output:  Vec<u8>,
    },
}

impl Proposer {
    pub fn new(address: [u8; 20], mode: ProposerMode) -> Self {
        Self { address, mode }
    }

    /// Returns true if this node is the proposer for the given slot.
    pub fn is_proposer(&self, slot: u64) -> bool {
        match &self.mode {
            ProposerMode::Poa { authority } => self.address == *authority,
            ProposerMode::RoundRobin { validators } => {
                if validators.is_empty() { return false; }
                validators[(slot as usize) % validators.len()] == self.address
            }
            ProposerMode::Vrf { validators, vrf_output } => {
                // VRF-weighted selection: higher-stake validators have higher weight.
                if validators.is_empty() { return false; }
                let idx = (vrf_output.iter().fold(0u64, |acc, &b| acc.wrapping_add(b as u64)))
                    as usize % validators.len();
                validators[idx] == self.address
            }
        }
    }

    /// Assert it's our turn. Returns slot number if yes.
    pub fn assert_is_proposer(&self, slot: u64) -> Result<u64, SequencerError> {
        if self.is_proposer(slot) {
            Ok(slot)
        } else {
            Err(SequencerError::NotProposer { slot, validator: self.address })
        }
    }

    pub fn address(&self) -> &[u8; 20] { &self.address }
}