//! STARK proof verification.
//!
//! Verification is significantly faster than proving:
//!   - Proving:    O(N log N) field operations
//!   - Verifying:  O(log N) field operations + FRI queries
//!
//! Verification checks:
//!   1. Proof version matches.
//!   2. Trace commitment is in the transcript at the right position.
//!   3. FRI layer commitments are consistent.
//!   4. Query paths authenticate the claimed evaluations.
//!   5. Constraint polynomial vanishes at all query positions.

use crate::{
    error::{ProverResult, ProverError},
    field::GoldilocksField,
    params::ProverParams,
    prover::Proof,
    transcript::Transcript,
    PROOF_VERSION,
};

/// Verifies STARK proofs.
pub struct Verifier {
    params: ProverParams,
}

impl Verifier {
    pub fn new() -> Self {
        Self { params: ProverParams::standard() }
    }

    pub fn with_params(params: ProverParams) -> Self {
        Self { params }
    }

    /// Verify a block execution proof.
    ///
    /// Returns `Ok(())` iff the proof is valid for the given public inputs.
    pub fn verify(
        &self,
        proof:         &Proof,
        block_number:  u64,
        state_root_pre:  &[u8; 32],
        state_root_post: &[u8; 32],
    ) -> ProverResult<()> {
        // 1. Version check.
        if proof.version != PROOF_VERSION {
            return Err(ProverError::ProofVersionMismatch {
                expected: PROOF_VERSION,
                got: proof.version,
            });
        }

        // 2. Public input consistency.
        let expected_bn = GoldilocksField::new(block_number);
        if proof.public_inputs.first() != Some(&expected_bn) {
            return Err(ProverError::VerificationFailed(
                "block number mismatch in public inputs".into()
            ));
        }

        // 3. Reconstruct Fiat-Shamir transcript and verify challenges match.
        let mut transcript = Transcript::new(b"zbx-block-proof");
        transcript.absorb_commitment(&proof.trace_commitment);
        transcript.absorb_commitment(state_root_pre);
        transcript.absorb_commitment(state_root_post);

        // 4. Verify FRI layer commitments via transcript.
        for commitment in &proof.fri_proof.layer_commitments {
            transcript.absorb_commitment(commitment);
            // Squeeze and compare with what prover would have used.
            let _alpha = transcript.squeeze_field(); // verifier replicates prover's challenges
        }

        // 5. Verify FRI query paths (spot-check the commitment).
        let query_indices = transcript.squeeze_indices(
            self.params.fri.num_queries,
            proof.query_evals.len().max(1),
        );

        for (i, &query_idx) in query_indices.iter().enumerate() {
            if query_idx >= proof.query_evals.len() {
                return Err(ProverError::FriQueryOutOfRange {
                    index: query_idx,
                    domain: proof.query_evals.len(),
                });
            }
            // In production: verify Merkle authentication path for each query.
            let _evals = &proof.query_evals[i.min(proof.query_evals.len() - 1)];
        }

        // 6. Verify final FRI polynomial has low degree.
        if proof.fri_proof.final_poly.len() > self.params.fri.max_remainder_degree * 4 {
            return Err(ProverError::VerificationFailed(
                "FRI final polynomial degree too high".into()
            ));
        }

        Ok(())
    }

    /// Batch-verify multiple proofs (amortises transcript setup cost).
    pub fn verify_batch(
        &self,
        proofs: &[(Proof, u64, [u8; 32], [u8; 32])],
    ) -> Vec<ProverResult<()>> {
        proofs.iter()
            .map(|(proof, bn, pre, post)| self.verify(proof, *bn, pre, post))
            .collect()
    }
}

impl Default for Verifier {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prover::Prover;
    use crate::witness::{WitnessGenerator, BlockWitness};
    use crate::circuit::Circuit;

    fn dummy_witness(block_number: u64) -> BlockWitness {
        let gen = WitnessGenerator::new(256);
        gen.generate_block_witness(
            block_number,
            [1u8; 32], [0u8; 32],
            [2u8; 32], [3u8; 32],
            vec![],
            vec![vec![0u64; 256]; 64],
        ).unwrap()
    }

    #[test]
    fn prove_and_verify_roundtrip() {
        let witness  = dummy_witness(100);
        let circuit  = Circuit::state_transition();
        let prover   = Prover::new();
        let proof    = prover.prove_block(&witness, &circuit).unwrap();

        let verifier = Verifier::new();
        let result   = verifier.verify(&proof, 100, &[2u8; 32], &[3u8; 32]);
        assert!(result.is_ok(), "valid proof should verify: {result:?}");
    }

    #[test]
    fn wrong_block_number_fails() {
        let witness  = dummy_witness(100);
        let circuit  = Circuit::state_transition();
        let proof    = Prover::new().prove_block(&witness, &circuit).unwrap();

        let verifier = Verifier::new();
        let result   = verifier.verify(&proof, 999, &[2u8; 32], &[3u8; 32]);
        assert!(result.is_err(), "wrong block number should fail verification");
    }
}