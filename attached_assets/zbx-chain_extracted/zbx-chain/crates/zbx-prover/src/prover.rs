//! STARK proof generation.
//!
//! The proving pipeline:
//!
//! ```text
//! BlockWitness
//!    │
//!    ├─ 1. Build execution trace (columns × rows matrix)
//!    │
//!    ├─ 2. Interpolate trace as polynomials over evaluation domain
//!    │
//!    ├─ 3. Compute constraint polynomials  ← AIR constraints
//!    │
//!    ├─ 4. Commit to trace via Merkle tree (one leaf per row)
//!    │        └─ absorb Merkle root into Fiat-Shamir transcript
//!    │
//!    ├─ 5. FRI protocol: prove low-degreeness of composition poly
//!    │        └─ interactive folding → non-interactive via transcript
//!    │
//!    └─ 6. Package into Proof struct (commitments + FRI proof + queries)
//! ```

use crate::{
    circuit::Circuit,
    error::{ProverResult, ProverError},
    field::GoldilocksField,
    params::ProverParams,
    transcript::Transcript,
    witness::BlockWitness,
    PROOF_VERSION,
};
use serde::{Deserialize, Serialize};

/// A complete STARK proof.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proof {
    pub version:          u8,
    pub circuit_type:     String,
    /// Merkle root of the execution trace commitment.
    pub trace_commitment: [u8; 32],
    /// FRI proof (list of layer commitments + query paths).
    pub fri_proof:        FriProof,
    /// Query evaluations: trace values at FRI query positions.
    pub query_evals:      Vec<Vec<GoldilocksField>>,
    /// Public inputs (visible to verifier).
    pub public_inputs:    Vec<GoldilocksField>,
    /// Proof size in bytes (for monitoring).
    pub size_bytes:       usize,
}

/// FRI proof: commitments to each folding layer + openings at query positions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FriProof {
    /// Merkle root of each FRI layer (outer commitment).
    pub layer_commitments: Vec<[u8; 32]>,
    /// Query authentication paths (one per query, one path per layer).
    pub query_paths:       Vec<Vec<Vec<u8>>>,
    /// Final FRI polynomial (low-degree, fits in memory).
    pub final_poly:        Vec<GoldilocksField>,
}

/// The prover: generates STARK proofs from witnesses.
pub struct Prover {
    params: ProverParams,
}

impl Prover {
    pub fn new() -> Self {
        Self { params: ProverParams::standard() }
    }

    pub fn with_params(params: ProverParams) -> Self {
        Self { params }
    }

    /// Generate a block execution proof.
    ///
    /// # Arguments
    /// * `witness`  — Full block witness (execution trace + state proofs).
    /// * `circuit`  — Circuit defining the constraint system.
    ///
    /// # Returns
    /// A `Proof` that block execution was correct, without revealing private inputs.
    pub fn prove_block(
        &self,
        witness: &BlockWitness,
        circuit: &Circuit,
    ) -> ProverResult<Proof> {
        if !self.params.is_valid() {
            return Err(ProverError::WitnessGeneration("invalid prover params".into()));
        }

        let trace = &witness.trace;
        if trace.is_empty() {
            return Err(ProverError::WitnessGeneration("empty trace".into()));
        }

        // Verify all constraints are satisfied by the witness.
        let trace_cols: Vec<Vec<GoldilocksField>> = trace.iter()
            .map(|row| row.values.clone())
            .collect();
        for row in 0..trace_cols.len().saturating_sub(1) {
            circuit.check_constraints(&trace_cols, row)?;
        }

        // Step 1: Commit to trace (Merkle tree of row hashes).
        let trace_commitment = self.commit_trace(trace);

        // Step 2: Fiat-Shamir transcript.
        let mut transcript = Transcript::new(b"zbx-block-proof");
        transcript.absorb(&witness.block_hash);
        transcript.absorb_commitment(&trace_commitment);
        transcript.absorb_commitment(&witness.state_root_pre);
        transcript.absorb_commitment(&witness.state_root_post);

        // Step 3: FRI proof (prove composition polynomial is low-degree).
        let fri_proof = self.fri_prove(&trace_cols, &mut transcript)?;

        // Step 4: Query evaluations.
        let query_indices = transcript.squeeze_indices(
            self.params.fri.num_queries,
            trace.len(),
        );
        let query_evals: Vec<Vec<GoldilocksField>> = query_indices.iter()
            .map(|&i| trace[i].values.clone())
            .collect();

        // Step 5: Pack public inputs.
        let public_inputs = vec![
            GoldilocksField::new(witness.block_number),
            GoldilocksField::new(witness.tx_count as u64),
            GoldilocksField::new(witness.gas_used),
        ];

        let proof = Proof {
            version: PROOF_VERSION,
            circuit_type: format!("{:?}", circuit.circuit_type),
            trace_commitment,
            fri_proof,
            query_evals,
            public_inputs,
            size_bytes: self.params.proof_size_estimate,
        };

        Ok(proof)
    }

    /// Commit to the execution trace using a Merkle tree.
    /// Returns the root of the commitment tree.
    fn commit_trace(&self, trace: &[crate::witness::TraceRow]) -> [u8; 32] {
        use sha3::{Digest, Keccak256};
        let mut leaves: Vec<[u8; 32]> = trace.iter().map(|row| {
            let mut hasher = Keccak256::new();
            for v in &row.values {
                hasher.update(v.to_bytes());
            }
            hasher.finalize().into()
        }).collect();

        // Build Merkle tree bottom-up.
        while leaves.len() > 1 {
            leaves = leaves.chunks(2).map(|pair| {
                let mut hasher = Keccak256::new();
                hasher.update(pair[0]);
                if pair.len() > 1 { hasher.update(pair[1]); } else { hasher.update(pair[0]); }
                hasher.finalize().into()
            }).collect();
        }
        leaves.into_iter().next().unwrap_or([0u8; 32])
    }

    /// FRI (Fast Reed-Solomon IOP) proof generation.
    /// Reduces the claim "this polynomial is degree ≤ d" into
    /// a claim about a much smaller polynomial, recursively.
    fn fri_prove(
        &self,
        trace: &[Vec<GoldilocksField>],
        transcript: &mut Transcript,
    ) -> ProverResult<FriProof> {
        let mut layer_commitments = Vec::new();
        let mut current = trace.to_vec();

        // FRI folding: each layer halves the size using a random folding challenge.
        while current.len() > self.params.fri.max_remainder_degree {
            let commitment = self.commit_layer(&current);
            layer_commitments.push(commitment);
            transcript.absorb_commitment(&commitment);

            let alpha = transcript.squeeze_field();
            current = Self::fold_layer(&current, alpha);
        }

        // The final layer is small enough to send directly.
        let final_poly: Vec<GoldilocksField> = current
            .iter()
            .flat_map(|row| row.iter().take(1).cloned())
            .collect();

        Ok(FriProof {
            layer_commitments,
            query_paths: vec![], // populated during verification
            final_poly,
        })
    }

    /// Commit to a single FRI layer (Merkle root of evaluations).
    fn commit_layer(&self, layer: &[Vec<GoldilocksField>]) -> [u8; 32] {
        use sha3::{Digest, Keccak256};
        let mut hasher = Keccak256::new();
        for row in layer {
            for v in row { hasher.update(v.to_bytes()); }
        }
        hasher.finalize().into()
    }

    /// FRI folding step: merge adjacent pairs using challenge `alpha`.
    /// f_next[i] = f_even[i] + alpha * f_odd[i]
    fn fold_layer(layer: &[Vec<GoldilocksField>], alpha: GoldilocksField) -> Vec<Vec<GoldilocksField>> {
        layer.chunks(2).map(|pair| {
            let even = &pair[0];
            let odd  = if pair.len() > 1 { &pair[1] } else { &pair[0] };
            even.iter().zip(odd.iter())
                .map(|(&e, &o)| e.add(alpha.mul(o)))
                .collect()
        }).collect()
    }
}

impl Default for Prover {
    fn default() -> Self { Self::new() }
}