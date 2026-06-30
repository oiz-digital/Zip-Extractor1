//! Fraud proofs — detect and prove incorrect state transitions.
//!
//! Used in two contexts:
//!   1. Bridge dispute: prover claims the bridge relayer submitted a wrong tx.
//!   2. Block challenge: validator claims a block producer executed incorrectly.
//!
//! Fraud proof protocol (optimistic style):
//!   1. Challenger posts a bond and claims "step S in block B is wrong".
//!   2. Bisection game: prover and challenger binary-search for the first
//!      diverging EVM execution step.
//!   3. The disputed single step is re-executed on-chain (or via ZK proof).
//!   4. If fraud is confirmed: block producer is slashed, challenger rewarded.
//!
//! ZK fraud proof (v0.2+):
//!   Instead of on-chain re-execution, a ZK proof of the correct output
//!   is submitted. Verification is O(1) on-chain (constant-size proof).

use crate::{
    error::{ProverResult, ProverError},
    prover::{Proof, Prover},
    circuit::Circuit,
    witness::BlockWitness,
};
use serde::{Deserialize, Serialize};

/// Type of fraud being claimed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FraudProofType {
    /// Block producer executed a transaction incorrectly.
    InvalidExecution {
        block_number:    u64,
        tx_index:        u32,
        diverging_step:  u64,
    },
    /// Bridge relayer minted tokens without a valid lock.
    BridgeMintWithoutLock {
        chain_id:    u64,
        lock_tx:     [u8; 32],
        mint_amount: u128,
    },
    /// Validator equivocated (signed two different blocks at same height).
    ValidatorEquivocation {
        validator:   [u8; 20],
        block_a:     [u8; 32],
        block_b:     [u8; 32],
        height:      u64,
    },
    /// State root mismatch between claimed and actual.
    StateRootMismatch {
        block_number:    u64,
        claimed_root:    [u8; 32],
        actual_root:     [u8; 32],
    },
}

/// A ZK fraud proof.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FraudProof {
    pub fraud_type:      FraudProofType,
    /// Correctness proof (ZK proof of the actual output).
    pub correctness_proof: Proof,
    /// Challenger's bond amount (in ZBX wei).
    pub bond:            u128,
    /// Block number when the dispute was opened.
    pub dispute_block:   u64,
    /// Deadline block (dispute must be resolved by this block).
    pub deadline_block:  u64,
}

/// Bisection game state (interactive fraud proof).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BisectionState {
    pub block_number:    u64,
    pub disputed_tx:     u32,
    pub left_step:       u64,   // lower bound of disputed range
    pub right_step:      u64,   // upper bound of disputed range
    pub challenger_claim: [u8; 32], // challenger's claimed intermediate state root
    pub prover_claim:    [u8; 32], // prover's claimed intermediate state root
    pub round:           u32,
}

impl BisectionState {
    /// The midpoint step to challenge next.
    pub fn midpoint(&self) -> u64 {
        (self.left_step + self.right_step) / 2
    }

    /// Narrow the bisection range based on claim at midpoint.
    pub fn narrow(
        &self,
        mid_step: u64,
        challenger_mid_root: [u8; 32],
        prover_mid_root:     [u8; 32],
    ) -> Self {
        if challenger_mid_root != prover_mid_root {
            // Disagreement is in [left, mid]: search left half.
            Self {
                right_step: mid_step,
                challenger_claim: challenger_mid_root,
                prover_claim:     prover_mid_root,
                round: self.round + 1,
                ..*self
            }
        } else {
            // Agreement at mid: disagreement is in [mid, right]: search right half.
            Self {
                left_step: mid_step,
                round: self.round + 1,
                ..*self
            }
        }
    }

    /// Returns true when bisection has narrowed to a single step.
    pub fn is_resolved(&self) -> bool {
        self.right_step - self.left_step <= 1
    }
}

/// Fraud proof generator.
pub struct FraudProver {
    prover: Prover,
}

impl FraudProver {
    pub fn new() -> Self {
        Self { prover: Prover::new() }
    }

    /// Generate a ZK fraud proof for a state root mismatch.
    /// Proves that the actual state root is `actual_root`, not `claimed_root`.
    pub fn prove_state_mismatch(
        &self,
        block_number:   u64,
        claimed_root:   [u8; 32],
        actual_root:    [u8; 32],
        witness:        &BlockWitness,
    ) -> ProverResult<FraudProof> {
        if witness.state_root_post == claimed_root {
            return Err(ProverError::NoFraudFound);
        }

        let circuit = Circuit::state_transition();
        let correctness_proof = self.prover.prove_block(witness, &circuit)?;

        Ok(FraudProof {
            fraud_type: FraudProofType::StateRootMismatch {
                block_number,
                claimed_root,
                actual_root,
            },
            correctness_proof,
            bond: 100 * 10u128.pow(18), // 100 ZBX bond
            dispute_block: block_number,
            deadline_block: block_number + 1000, // ~83 minutes at 5s blocks
        })
    }
}

impl Default for FraudProver {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bisection_narrows_correctly() {
        let mut state = BisectionState {
            block_number: 100,
            disputed_tx: 5,
            left_step: 0,
            right_step: 1024,
            challenger_claim: [0xAAu8; 32],
            prover_claim:     [0xBBu8; 32],
            round: 0,
        };

        // Midpoint = 512
        assert_eq!(state.midpoint(), 512);

        // Challenger and prover agree at 512 → dispute is in [512, 1024]
        let agreed_root = [0xCCu8; 32];
        state = state.narrow(512, agreed_root, agreed_root);
        assert_eq!(state.left_step, 512);
        assert_eq!(state.right_step, 1024);
        assert_eq!(state.round, 1);
    }

    #[test]
    fn bisection_resolves_at_single_step() {
        let state = BisectionState {
            block_number: 100,
            disputed_tx: 0,
            left_step: 5,
            right_step: 6,
            challenger_claim: [0xAAu8; 32],
            prover_claim:     [0xBBu8; 32],
            round: 10,
        };
        assert!(state.is_resolved(), "single step should be resolved");
    }
}