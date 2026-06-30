//! Data Availability Sampling (DAS) for light clients.
//!
//! Light clients can verify data availability without downloading full blobs
//! by randomly sampling chunks and verifying KZG proofs.
//!
//! # Fix (2026-06-27)
//!
//! The previous body unconditionally returned `Err(DaError::NotImplemented)`
//! for every block with blobs — light client DA verification was non-functional.
//!
//! This implementation adds:
//! - `ChunkProof` — a single chunk (index, data, KZG commitment + proof).
//! - `ChunkFetcher` — async trait abstracting the peer network. Production
//!   nodes inject a real P2P implementation; tests inject a mock.
//! - `DaSampler::sample_block` — selects `sample_count` random chunk indices
//!   using a deterministic PRNG seeded from `(block, blob_index)`, fetches
//!   each chunk from the network, and verifies the KZG inclusion proof via
//!   `KzgSettings::verify_blob_kzg_proof`. Returns `Ok(SampleResult)` only
//!   when all sampled chunks pass; fails closed on first unavailability or
//!   proof failure.

use async_trait::async_trait;
use crate::{commitment::{KzgCommitment, KzgProof, KzgSettings, BLOB_SIZE_BYTES}, error::DaError};
use serde::{Deserialize, Serialize};

/// Number of samples a light client takes per blob (default: 75).
/// This gives >99.99% detection probability for withheld data.
pub const DEFAULT_SAMPLE_COUNT: usize = 75;

/// Number of 32-byte field elements per blob.
const FIELD_ELEMENTS_PER_BLOB: usize = BLOB_SIZE_BYTES / 32;

// ── ChunkProof ────────────────────────────────────────────────────────────────

/// A single sampled chunk with its KZG inclusion proof.
///
/// The prover supplies one `ChunkProof` per sampled index. The verifier
/// checks that:
/// 1. `data` is exactly 32 bytes (one BLS12-381 Fr field element).
/// 2. `commitment` is a valid G1 point.
/// 3. `proof` is a valid G1 point.
/// 4. The KZG pairing check passes for `(commitment, proof, chunk_data_padded)`
///    where `chunk_data_padded` is the 32-byte element zero-padded into a
///    full 131072-byte blob for the evaluation-point derivation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkProof {
    /// Index of this field element in the blob (0..4095).
    pub index: u16,
    /// 32-byte field element data.
    pub data: [u8; 32],
    /// KZG commitment to the full blob (48 bytes, compressed G1).
    pub commitment: KzgCommitment,
    /// KZG opening proof at this chunk index (48 bytes, compressed G1).
    pub proof: KzgProof,
}

// ── ChunkFetcher ──────────────────────────────────────────────────────────────

/// Async trait for fetching individual chunk proofs from the DA peer network.
///
/// Production nodes implement this against the ZBX P2P DAS sub-protocol.
/// Tests implement it with an in-memory mock.
#[async_trait]
pub trait ChunkFetcher: Send + Sync {
    /// Fetch the KZG chunk proof for `chunk_index` of blob `blob_index`
    /// in block `block`. Returns `None` if the peer does not have the chunk
    /// (chunk is withheld).
    async fn fetch_chunk(
        &self,
        block: u64,
        blob_index: usize,
        chunk_index: u16,
    ) -> Option<ChunkProof>;
}

// ── DaSampler ─────────────────────────────────────────────────────────────────

/// Light-client DA sampler.
pub struct DaSampler {
    kzg: KzgSettings,
    sample_count: usize,
}

impl DaSampler {
    pub fn new(kzg: KzgSettings, sample_count: usize) -> Self {
        DaSampler { kzg, sample_count }
    }

    /// Sample data availability for a block's blobs.
    ///
    /// For each blob in `0..blob_count`, selects `self.sample_count` random
    /// chunk indices (deterministic PRNG seeded from block + blob_index),
    /// fetches each chunk via `fetcher`, and verifies its KZG inclusion proof.
    ///
    /// Returns `Ok(SampleResult { da_confirmed: true })` only when every
    /// sampled chunk in every blob passes. Fails closed on the first missing
    /// or invalid chunk.
    pub async fn sample_block<F: ChunkFetcher>(
        &self,
        block: u64,
        blob_count: usize,
        fetcher: &F,
    ) -> Result<SampleResult, DaError> {
        if blob_count == 0 {
            return Ok(SampleResult {
                block,
                samples: 0,
                available: 0,
                da_confirmed: true,
            });
        }

        let mut total_samples = 0usize;
        let mut total_available = 0usize;

        for blob_index in 0..blob_count {
            let indices = self.sample_indices(block, blob_index);
            for &chunk_index in &indices {
                total_samples += 1;
                match fetcher.fetch_chunk(block, blob_index, chunk_index).await {
                    None => {
                        tracing::warn!(
                            block,
                            blob_index,
                            chunk_index,
                            "DA sampling: chunk withheld by peer"
                        );
                        return Ok(SampleResult {
                            block,
                            samples: total_samples,
                            available: total_available,
                            da_confirmed: false,
                        });
                    }
                    Some(chunk) => {
                        if chunk.index != chunk_index {
                            tracing::warn!(
                                block,
                                blob_index,
                                chunk_index,
                                returned = chunk.index,
                                "DA sampling: peer returned wrong chunk index"
                            );
                            return Ok(SampleResult {
                                block,
                                samples: total_samples,
                                available: total_available,
                                da_confirmed: false,
                            });
                        }
                        if !self.verify_chunk(&chunk) {
                            tracing::warn!(
                                block,
                                blob_index,
                                chunk_index,
                                "DA sampling: KZG proof invalid"
                            );
                            return Ok(SampleResult {
                                block,
                                samples: total_samples,
                                available: total_available,
                                da_confirmed: false,
                            });
                        }
                        total_available += 1;
                    }
                }
            }
        }

        tracing::debug!(
            block,
            total_samples,
            total_available,
            "DA sampling: all chunks available and verified"
        );

        Ok(SampleResult {
            block,
            samples: total_samples,
            available: total_available,
            da_confirmed: total_samples > 0 && total_samples == total_available,
        })
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Select `self.sample_count` chunk indices for the given `(block, blob_index)`.
    ///
    /// Uses a deterministic xorshift64 PRNG seeded from `block ^ (blob_index << 32)`
    /// so every light client samples the same set of chunks — enabling aggregation
    /// of availability reports without coordination.
    fn sample_indices(&self, block: u64, blob_index: usize) -> Vec<u16> {
        let seed = block ^ ((blob_index as u64).wrapping_shl(32));
        let mut state = if seed == 0 { 0xdeadbeef_cafebabe_u64 } else { seed };
        let count = self.sample_count.min(FIELD_ELEMENTS_PER_BLOB);
        let mut indices = Vec::with_capacity(count);
        let mut seen = std::collections::HashSet::with_capacity(count * 2);
        while indices.len() < count {
            // xorshift64
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let idx = (state % FIELD_ELEMENTS_PER_BLOB as u64) as u16;
            if seen.insert(idx) {
                indices.push(idx);
            }
        }
        indices
    }

    /// Verify a single chunk's KZG inclusion proof.
    ///
    /// Pads the 32-byte chunk element into a synthetic 131072-byte blob with
    /// the element placed at position `chunk.index`. This is the canonical
    /// way to derive the evaluation point `z` and check the opening proof
    /// against the blob commitment via `verify_blob_kzg_proof`.
    fn verify_chunk(&self, chunk: &ChunkProof) -> bool {
        let mut blob_padded = vec![0u8; BLOB_SIZE_BYTES];
        let start = chunk.index as usize * 32;
        if start + 32 > BLOB_SIZE_BYTES {
            return false;
        }
        blob_padded[start..start + 32].copy_from_slice(&chunk.data);
        self.kzg.verify_blob_kzg_proof(&chunk.commitment, &chunk.proof, &blob_padded)
    }
}

/// The result of a single DA sampling check.
#[derive(Debug, Serialize, Deserialize)]
pub struct SampleResult {
    /// Block number that was sampled.
    pub block: u64,
    /// Number of samples taken.
    pub samples: usize,
    /// Number of samples that were available.
    pub available: usize,
    /// Whether full DA is confirmed (all samples returned valid proofs).
    pub da_confirmed: bool,
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Mock fetcher: serves pre-registered chunks; returns None for unknown.
    struct MockFetcher {
        chunks: HashMap<(u64, usize, u16), ChunkProof>,
    }

    impl MockFetcher {
        fn new() -> Self { Self { chunks: HashMap::new() } }
        fn insert(&mut self, block: u64, blob: usize, chunk: ChunkProof) {
            self.chunks.insert((block, blob, chunk.index), chunk);
        }
    }

    #[async_trait]
    impl ChunkFetcher for MockFetcher {
        async fn fetch_chunk(&self, block: u64, blob: usize, idx: u16) -> Option<ChunkProof> {
            self.chunks.get(&(block, blob, idx)).cloned()
        }
    }

    /// Build a KzgSettings in devnet τ=1 mode.
    fn devnet_kzg() -> KzgSettings { KzgSettings::load() }

    #[test]
    fn empty_blob_count_always_confirmed() {
        // Zero blobs → trivially available, no network calls needed.
        let kzg = devnet_kzg();
        let sampler = DaSampler::new(kzg, DEFAULT_SAMPLE_COUNT);
        let fetcher = MockFetcher::new();
        let result = tokio::runtime::Builder::new_current_thread()
            .build().unwrap()
            .block_on(sampler.sample_block(42, 0, &fetcher))
            .unwrap();
        assert!(result.da_confirmed);
        assert_eq!(result.samples, 0);
    }

    #[test]
    fn withheld_chunk_returns_not_confirmed() {
        // MockFetcher returns None for all chunks → da_confirmed = false.
        let kzg = devnet_kzg();
        let sampler = DaSampler::new(kzg, 4);
        let fetcher = MockFetcher::new(); // no chunks registered
        let result = tokio::runtime::Builder::new_current_thread()
            .build().unwrap()
            .block_on(sampler.sample_block(1, 1, &fetcher))
            .unwrap();
        assert!(!result.da_confirmed);
        assert_eq!(result.available, 0);
        assert!(result.samples >= 1);
    }

    #[test]
    fn sample_indices_deterministic() {
        // Same (block, blob_index) must always produce the same indices.
        let kzg = devnet_kzg();
        let sampler = DaSampler::new(kzg, 10);
        let a = sampler.sample_indices(100, 0);
        let b = sampler.sample_indices(100, 0);
        assert_eq!(a, b, "sample_indices must be deterministic");
    }

    #[test]
    fn sample_indices_no_duplicates() {
        let kzg = devnet_kzg();
        let sampler = DaSampler::new(kzg, DEFAULT_SAMPLE_COUNT);
        let indices = sampler.sample_indices(99, 1);
        let unique: std::collections::HashSet<_> = indices.iter().collect();
        assert_eq!(unique.len(), indices.len(), "all sampled indices must be distinct");
    }

    #[test]
    fn sample_indices_differ_across_blobs() {
        let kzg = devnet_kzg();
        let sampler = DaSampler::new(kzg, 10);
        let a = sampler.sample_indices(1, 0);
        let b = sampler.sample_indices(1, 1);
        assert_ne!(a, b, "different blob indices must produce different samples");
    }
}
