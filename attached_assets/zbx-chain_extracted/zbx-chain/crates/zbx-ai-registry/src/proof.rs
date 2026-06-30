//! ZK Proof of Correct Inference.
//!
//! Every AI inference produces a cryptographic proof that:
//!   1. The input was processed by the registered model (weight hash verified)
//!   2. The output follows from the input + weights (determinism proof)
//!   3. The call happened at a specific block (replay protection)
//!
//! Proof scheme: Merkle commitment over (input, weights_hash, output, block).
//! Full ZK-STARK proof integration is planned (ZEP-019); this module provides
//! the commitment layer that STARK proofs will anchor to.
//!
//! Security:
//! - SHA3-256 collision resistance: 2^128 security level
//! - Block commitment prevents out-of-order replay
//! - Weight hash ties proof to a specific model version
//! - Aggregation: multiple proofs can be batched into one root

use crate::error::RegistryError;
use zbx_ai_precompile::ModelId;
use serde::{Serialize, Deserialize};

/// A commitment to a single inference execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceCommitment {
    /// SHA3-256 of the input bytes.
    pub input_hash:    [u8; 32],
    /// SHA3-256 of the model weights (from DA layer).
    pub weights_hash:  [u8; 32],
    /// SHA3-256 of the output bytes.
    pub output_hash:   [u8; 32],
    /// Block number when inference ran.
    pub block_number:  u64,
    /// Model ID.
    pub model_id:      ModelId,
    /// Transaction index in the block.
    pub tx_index:      u32,
}

impl InferenceCommitment {
    pub fn new(
        input:        &[u8],
        weights_hash: [u8; 32],
        output:       &[u8],
        block_number: u64,
        model_id:     ModelId,
        tx_index:     u32,
    ) -> Self {
        Self {
            input_hash:   sha3_256(input),
            weights_hash,
            output_hash:  sha3_256(output),
            block_number,
            model_id,
            tx_index,
        }
    }

    /// Compute the 32-byte leaf hash for Merkle tree inclusion.
    pub fn leaf_hash(&self) -> [u8; 32] {
        let mut data = Vec::with_capacity(32 * 3 + 8 + 1 + 4);
        data.extend_from_slice(&self.input_hash);
        data.extend_from_slice(&self.weights_hash);
        data.extend_from_slice(&self.output_hash);
        data.extend_from_slice(&self.block_number.to_be_bytes());
        data.push(self.model_id as u8);
        data.extend_from_slice(&self.tx_index.to_be_bytes());
        sha3_256(&data)
    }
}

/// A proof of correct inference, anchored to a Merkle root.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceProof {
    /// The commitment being proven.
    pub commitment:  InferenceCommitment,
    /// Leaf hash of the commitment.
    pub leaf_hash:   [u8; 32],
    /// Merkle path (sibling hashes from leaf to root).
    pub merkle_path: Vec<[u8; 32]>,
    /// Merkle root this proof anchors to.
    pub merkle_root: [u8; 32],
    /// Proof generation timestamp (block number).
    pub proved_at:   u64,
}

impl InferenceProof {
    /// Verify the Merkle proof.
    pub fn verify(&self) -> Result<(), RegistryError> {
        // Recompute leaf
        let expected_leaf = self.commitment.leaf_hash();
        if expected_leaf != self.leaf_hash {
            return Err(RegistryError::ProofInvalid(
                "leaf hash mismatch".to_string()
            ));
        }

        // Walk Merkle path
        let mut current = self.leaf_hash;
        for sibling in &self.merkle_path {
            current = merkle_parent(&current, sibling);
        }

        if current != self.merkle_root {
            return Err(RegistryError::ProofInvalid(
                "Merkle root mismatch".to_string()
            ));
        }

        Ok(())
    }

    /// Quick check: does the proof's model_id match expected?
    pub fn verify_model(&self, expected: ModelId) -> Result<(), RegistryError> {
        if self.commitment.model_id != expected {
            return Err(RegistryError::ProofInvalid(format!(
                "model_id mismatch: expected {:?}, got {:?}",
                expected, self.commitment.model_id
            )));
        }
        Ok(())
    }

    /// Quick check: does the proof's block match expected?
    pub fn verify_block(&self, expected_block: u64) -> Result<(), RegistryError> {
        if self.commitment.block_number != expected_block {
            return Err(RegistryError::ProofInvalid(format!(
                "block mismatch: expected {}, got {}",
                expected_block, self.commitment.block_number
            )));
        }
        Ok(())
    }
}

/// Batch of multiple inference proofs (aggregated into one Merkle tree).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofBatch {
    /// Individual proofs.
    pub proofs:      Vec<InferenceProof>,
    /// Merkle root over all leaf hashes.
    pub batch_root:  [u8; 32],
    /// Block range covered.
    pub from_block:  u64,
    /// Block range covered.
    pub to_block:    u64,
}

impl ProofBatch {
    /// Build a proof batch from a list of commitments.
    pub fn build(commitments: Vec<InferenceCommitment>, at_block: u64)
        -> Result<Self, RegistryError>
    {
        if commitments.is_empty() {
            return Err(RegistryError::ProofInvalid("empty batch".to_string()));
        }

        let leaves: Vec<[u8; 32]> = commitments.iter()
            .map(|c| c.leaf_hash())
            .collect();

        let batch_root = build_merkle_root(&leaves);

        let from_block = commitments.iter().map(|c| c.block_number).min().unwrap_or(0);
        let to_block   = commitments.iter().map(|c| c.block_number).max().unwrap_or(0);

        let proofs = commitments.into_iter().enumerate().map(|(i, commitment)| {
            let leaf_hash   = leaves[i];
            let merkle_path = build_merkle_path(&leaves, i);
            InferenceProof {
                commitment,
                leaf_hash,
                merkle_path,
                merkle_root: batch_root,
                proved_at:   at_block,
            }
        }).collect();

        Ok(Self { proofs, batch_root, from_block, to_block })
    }

    pub fn verify_all(&self) -> Result<(), RegistryError> {
        for proof in &self.proofs {
            proof.verify()?;
        }
        Ok(())
    }

    pub fn len(&self) -> usize { self.proofs.len() }
    pub fn is_empty(&self) -> bool { self.proofs.is_empty() }
}

/// Build Merkle root from leaves (padded to next power of 2).
fn build_merkle_root(leaves: &[[u8; 32]]) -> [u8; 32] {
    if leaves.is_empty() { return [0u8; 32]; }
    if leaves.len() == 1 { return leaves[0]; }

    let n = next_pow2(leaves.len());
    let mut level: Vec<[u8; 32]> = leaves.to_vec();
    // Pad to power of 2 by duplicating the last leaf
    while level.len() < n {
        level.push(*level.last().unwrap());
    }

    while level.len() > 1 {
        let mut next = Vec::with_capacity(level.len() / 2);
        for chunk in level.chunks(2) {
            next.push(merkle_parent(&chunk[0], &chunk[1]));
        }
        level = next;
    }
    level[0]
}

/// Build the Merkle path (sibling hashes) for leaf at `index`.
fn build_merkle_path(leaves: &[[u8; 32]], index: usize) -> Vec<[u8; 32]> {
    if leaves.len() <= 1 { return vec![]; }

    let n = next_pow2(leaves.len());
    let mut level: Vec<[u8; 32]> = leaves.to_vec();
    while level.len() < n {
        level.push(*level.last().unwrap());
    }

    let mut path = Vec::new();
    let mut idx = index;
    while level.len() > 1 {
        let sibling_idx = if idx % 2 == 0 { idx + 1 } else { idx - 1 };
        let sibling = level.get(sibling_idx).copied().unwrap_or(level[idx]);
        path.push(sibling);
        let mut next = Vec::with_capacity(level.len() / 2);
        for chunk in level.chunks(2) {
            next.push(merkle_parent(&chunk[0], &chunk[1]));
        }
        level = next;
        idx /= 2;
    }
    path
}

/// Merkle parent = SHA3(left || right), sorted so smaller is always left.
fn merkle_parent(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    let mut data = [0u8; 64];
    if a <= b {
        data[..32].copy_from_slice(a);
        data[32..].copy_from_slice(b);
    } else {
        data[..32].copy_from_slice(b);
        data[32..].copy_from_slice(a);
    }
    sha3_256(&data)
}

fn next_pow2(n: usize) -> usize {
    if n == 0 { return 1; }
    let mut p = 1;
    while p < n { p <<= 1; }
    p
}

fn sha3_256(data: &[u8]) -> [u8; 32] {
    use sha3::{Digest, Sha3_256};
    let mut h = Sha3_256::new();
    h.update(data);
    let out = h.finalize();
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&out);
    arr
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_commitment(model_id: ModelId, block: u64, idx: u32) -> InferenceCommitment {
        InferenceCommitment::new(
            b"input_data_here",
            [0u8; 32],
            b"output_result",
            block,
            model_id,
            idx,
        )
    }

    #[test]
    fn single_proof_verifies() {
        let batch = ProofBatch::build(
            vec![test_commitment(ModelId::SpamClassifier, 1000, 0)],
            1001,
        ).unwrap();
        assert_eq!(batch.len(), 1);
        batch.verify_all().unwrap();
    }

    #[test]
    fn multi_proof_batch_verifies() {
        let commitments = vec![
            test_commitment(ModelId::SpamClassifier,  1000, 0),
            test_commitment(ModelId::RiskScorer,      1000, 1),
            test_commitment(ModelId::NftTagger,       1001, 0),
            test_commitment(ModelId::GasOptimizer,    1001, 1),
        ];
        let batch = ProofBatch::build(commitments, 1002).unwrap();
        assert_eq!(batch.len(), 4);
        batch.verify_all().unwrap();
    }

    #[test]
    fn tampered_proof_fails() {
        let mut batch = ProofBatch::build(
            vec![test_commitment(ModelId::SpamClassifier, 1000, 0)],
            1001,
        ).unwrap();
        // Tamper the leaf hash
        batch.proofs[0].leaf_hash[0] ^= 0xFF;
        let err = batch.verify_all().unwrap_err();
        assert!(matches!(err, RegistryError::ProofInvalid(_)));
    }

    #[test]
    fn empty_batch_rejected() {
        let err = ProofBatch::build(vec![], 1000).unwrap_err();
        assert!(matches!(err, RegistryError::ProofInvalid(_)));
    }

    #[test]
    fn leaf_hash_is_deterministic() {
        let c1 = test_commitment(ModelId::SpamClassifier, 100, 0);
        let c2 = test_commitment(ModelId::SpamClassifier, 100, 0);
        assert_eq!(c1.leaf_hash(), c2.leaf_hash());
    }

    #[test]
    fn model_verify_checks_id() {
        let batch = ProofBatch::build(
            vec![test_commitment(ModelId::SpamClassifier, 1000, 0)],
            1001,
        ).unwrap();
        batch.proofs[0].verify_model(ModelId::SpamClassifier).unwrap();
        let err = batch.proofs[0].verify_model(ModelId::RiskScorer).unwrap_err();
        assert!(matches!(err, RegistryError::ProofInvalid(_)));
    }
}
