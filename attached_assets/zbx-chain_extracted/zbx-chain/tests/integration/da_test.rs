//! Integration tests for the ZBX Data Availability layer.

#[cfg(test)]
mod da_tests {
    use zbx_da::{
        blob::{Blob, BlobTransaction, BlobSidecar},
        commitment::{KzgCommitment, KzgProof, KzgSettings},
        sampling::{DaSampler, DEFAULT_SAMPLE_COUNT},
        store::BlobStore,
        pruner::BlobPruner,
        BLOB_SIZE, MAX_BLOBS_PER_BLOCK, BLOB_PRUNE_BLOCKS,
    };
    use zbx_types::CHAIN_ID_MAINNET;
    use std::sync::Arc;

    // ── Blob creation ──────────────────────────────────────────────────────

    #[test]
    fn test_blob_from_bytes() {
        let data = b"Hello, ZBX DA layer!";
        let blob = Blob::from_bytes(data).unwrap();
        assert_eq!(&blob.0[..data.len()], data);
        assert_eq!(&blob.0[data.len()..], &vec![0u8; BLOB_SIZE - data.len()][..]);
    }

    #[test]
    fn test_blob_too_large() {
        let too_large = vec![0u8; BLOB_SIZE + 1];
        let result = Blob::from_bytes(&too_large);
        assert!(result.is_err());
    }

    #[test]
    fn test_blob_versioned_hash() {
        let blob = Blob::zeroed();
        let hash = blob.versioned_hash();
        assert_eq!(hash[0], 0x01, "versioned hash must start with 0x01");
        assert_ne!(hash, [0u8; 32]);
    }

    // ── KZG commitments ───────────────────────────────────────────────────

    #[test]
    fn test_kzg_commitment_creation() {
        let settings = KzgSettings::load();
        assert!(settings.loaded);
        let blob = Blob::from_bytes(b"ZBX KZG test blob").unwrap();
        let commitment = settings.blob_to_kzg_commitment(&blob.0[..]);
        assert_ne!(commitment.0, [0u8; 48]);
        assert_eq!(commitment.0[0], 0xc0); // dev mode marker
    }

    #[test]
    fn test_kzg_proof_verification() {
        let settings = KzgSettings::load();
        let blob = Blob::from_bytes(b"verify me").unwrap();
        let commitment = settings.blob_to_kzg_commitment(&blob.0[..]);
        let proof = KzgProof([0u8; 48]);
        assert!(settings.verify_blob_kzg_proof(&commitment, &proof, &blob.0[..]));
    }

    // ── Blob transaction validation ───────────────────────────────────────

    #[test]
    fn test_blob_tx_valid() {
        let settings = KzgSettings::load();
        let blob = Blob::from_bytes(b"rollup batch data").unwrap();
        let hash = blob.versioned_hash();
        let commitment = settings.blob_to_kzg_commitment(&blob.0[..]);
        let proof = KzgProof([0u8; 48]);
        let sidecar = BlobSidecar { blob, commitment, proof };

        let tx = BlobTransaction {
            chain_id: CHAIN_ID_MAINNET,
            nonce: 0,
            max_fee_per_gas: 1_000_000_000,
            max_priority_fee_per_gas: 100_000_000,
            max_fee_per_blob_gas: 100,
            to: [0u8; 20],
            value: 0,
            input: vec![],
            blob_versioned_hashes: vec![hash],
            sidecars: vec![sidecar],
        };

        assert!(tx.validate_sidecars().is_ok());
    }

    #[test]
    fn test_blob_tx_too_many_blobs() {
        let blobs = (0..=MAX_BLOBS_PER_BLOCK).map(|_| {
            let blob = Blob::zeroed();
            let hash = blob.versioned_hash();
            let sidecar = BlobSidecar {
                blob,
                commitment: KzgCommitment([0u8; 48]),
                proof: KzgProof([0u8; 48]),
            };
            (hash, sidecar)
        }).collect::<Vec<_>>();

        let tx = BlobTransaction {
            chain_id: CHAIN_ID_MAINNET,
            nonce: 0,
            max_fee_per_gas: 0,
            max_priority_fee_per_gas: 0,
            max_fee_per_blob_gas: 0,
            to: [0u8; 20],
            value: 0,
            input: vec![],
            blob_versioned_hashes: blobs.iter().map(|(h, _)| *h).collect(),
            sidecars: blobs.into_iter().map(|(_, s)| s).collect(),
        };

        assert!(tx.validate_sidecars().is_err());
    }

    // ── Blob store ────────────────────────────────────────────────────────

    #[test]
    fn test_blob_store_insert_and_retrieve() {
        let store = BlobStore::new();
        let blob = Blob::from_bytes(b"store test").unwrap();
        let hash = blob.versioned_hash();
        let sidecar = BlobSidecar {
            blob,
            commitment: KzgCommitment([0u8; 48]),
            proof: KzgProof([0u8; 48]),
        };

        store.insert(hash, sidecar).unwrap();
        assert!(store.contains(&hash));
        assert!(store.get(&hash).is_some());
    }

    // ── DA sampling ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_da_sampling_no_blobs() {
        let kzg = KzgSettings::load();
        let sampler = DaSampler::new(kzg, DEFAULT_SAMPLE_COUNT);
        let result = sampler.sample_block(100, 0).await.unwrap();
        assert!(result.da_confirmed);
        assert_eq!(result.samples, 0);
    }

    #[tokio::test]
    async fn test_da_sampling_with_blobs() {
        let kzg = KzgSettings::load();
        let sampler = DaSampler::new(kzg, DEFAULT_SAMPLE_COUNT);
        let result = sampler.sample_block(200, 4).await.unwrap();
        assert!(result.da_confirmed);
        assert!(result.samples > 0);
    }

    // ── Blob pruner ───────────────────────────────────────────────────────

    #[test]
    fn test_blob_pruner_no_prune_before_window() {
        let store = Arc::new(BlobStore::new());
        let mut pruner = BlobPruner::new(store.clone());
        // Insert a blob at block 100
        let blob = Blob::zeroed();
        let hash = blob.versioned_hash();
        store.insert(hash, BlobSidecar {
            blob,
            commitment: KzgCommitment([0u8; 48]),
            proof: KzgProof([0u8; 48]),
        }).unwrap();
        pruner.register_block(100, vec![hash]);
        // Finalize at block 200 (not past prune window)
        pruner.register_block(200, vec![]);
        pruner.prune();
        // Blob should still be there (200 - 100 < BLOB_PRUNE_BLOCKS)
        assert!(store.contains(&hash));
    }
}