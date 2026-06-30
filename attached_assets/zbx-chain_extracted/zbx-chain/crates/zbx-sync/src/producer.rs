//! End-to-end snapshot producer: raw chunks → BLS-quorum-signed
//! `SnapshotMeta`. Per-chunk MPT roots are computed with the same
//! `MutableTrie<MemoryTrieDB>` codec the consumer's `verify_chunk`
//! uses, so producer↔consumer agreement on `chunk_roots[i]` is
//! guaranteed by construction.

use crate::coordinator::SnapshotMeta;
use crate::error::SyncError;
use crate::manifest::build_signed_manifest;
use zbx_trie::{trie::MemoryTrieDB, MutableTrie};
use zbx_types::H256;

/// End-to-end producer entry point: consumes the raw per-chunk leaf
/// sets, derives every chunk's MPT root with the SAME `MutableTrie`
/// codec the consumer's `verify_chunk` uses, BLS-signs the canonical
/// manifest digest with the supplied signers, and returns the full
/// [`SnapshotMeta`] ready to ship to a snap-sync peer.
///
/// `signer_indices` are committee positions of the signing
/// validators; `signer_secret_keys` are their BLS secret keys in the
/// same order. `n_validators` is the full committee size (used to
/// size the `ValidatorBitmap`).
///
/// # Errors
///
/// * `SyncError::Interrupted` if the chunk list is empty, if any
///   per-chunk MPT computation fails, or if the signer-index /
///   secret-key arities disagree.
/// * `SyncError::BadManifestSignature` if BLS aggregation fails.
pub fn produce_signed_snapshot(
    pivot_height: u64,
    state_root: H256,
    chunks: &[Vec<(H256, Vec<u8>)>],
    n_validators: usize,
    signer_indices: &[usize],
    signer_secret_keys: &[[u8; 32]],
) -> Result<SnapshotMeta, SyncError> {
    if chunks.is_empty() {
        return Err(SyncError::Interrupted(
            "produce_signed_snapshot: empty chunks vector".into(),
        ));
    }
    let chunk_roots: Vec<H256> = chunks
        .iter()
        .enumerate()
        .map(|(i, leaves)| compute_chunk_mpt_root(i, leaves))
        .collect::<Result<Vec<_>, _>>()?;

    let signed = build_signed_manifest(
        pivot_height,
        state_root,
        &chunk_roots,
        n_validators,
        signer_indices,
        signer_secret_keys,
    )?;

    Ok(SnapshotMeta {
        pivot_height,
        state_root,
        chunk_roots,
        chunk_root: signed.chunk_root,
        bls_quorum_sig: signed.bls_quorum_sig,
        bls_signers: signed.bls_signers,
    })
}

/// Build a single chunk's MPT and return its root, using the same
/// `MutableTrie<MemoryTrieDB>` instantiation the consumer's
/// `snap_sync::verify_chunk` uses. Producer-vs-consumer codec
/// agreement is therefore guaranteed by construction.
fn compute_chunk_mpt_root(
    chunk_id: usize,
    leaves: &[(H256, Vec<u8>)],
) -> Result<H256, SyncError> {
    if leaves.is_empty() {
        return Err(SyncError::Interrupted(format!(
            "produce_signed_snapshot: chunk {chunk_id} is empty"
        )));
    }
    let mut trie = MutableTrie::new(MemoryTrieDB::default());
    for (key, value) in leaves {
        trie.insert(key.as_bytes(), value.clone())
            .map_err(|e| SyncError::Interrupted(format!("trie insert chunk {chunk_id}: {e}")))?;
    }
    trie.commit()
        .map_err(|e| SyncError::Interrupted(format!("trie commit chunk {chunk_id}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinator::SyncCoordinator;
    use crate::manifest::{build_inclusion_proofs, verify_chunk_inclusion, verify_manifest_signature};
    use std::sync::Arc;
    use zbx_crypto::bls as ckbls;
    use zbx_threshold::BlsPubKey;
    use zbx_types::address::Address;

    fn key(b: u8) -> H256 {
        let mut x = [0u8; 32];
        x[31] = b;
        H256(x)
    }

    fn sk_from(seed: u8) -> [u8; 32] {
        let mut k = [0u8; 32];
        for (i, b) in k.iter_mut().enumerate() {
            *b = seed.wrapping_add(i as u8 + 1);
        }
        k
    }

    fn pk_from(sk: &[u8; 32]) -> BlsPubKey {
        let p = ckbls::BlsPrivKey::from_bytes(sk).unwrap();
        BlsPubKey(*p.to_pubkey().as_bytes())
    }

    /// End-to-end producer ↔ consumer roundtrip: producer signs a real
    /// 2-chunk snapshot with a 3-of-4 BLS quorum, consumer verifies
    /// the manifest signature + chunk_root binding without any
    /// fixture-side surgery. Closes the "producer integration is just
    /// a helper" gap from architect-review v2.
    #[test]
    fn producer_roundtrip_consumer_verifies_3_of_4() {
        let sks: Vec<[u8; 32]> = (0..4u8).map(sk_from).collect();
        let pks: Vec<BlsPubKey> = sks.iter().map(pk_from).collect();
        let validator_keys: Vec<(Address, BlsPubKey)> = pks
            .iter()
            .enumerate()
            .map(|(i, pk)| (Address([i as u8; 20]), pk.clone()))
            .collect();

        let chunk0: Vec<(H256, Vec<u8>)> =
            (0..16u8).map(|i| (key(i), vec![0xA0, i])).collect();
        let chunk1: Vec<(H256, Vec<u8>)> =
            (16..32u8).map(|i| (key(i), vec![0xB0, i])).collect();
        let chunks = vec![chunk0, chunk1];
        let state_root = key(0xEE);

        let signers: Vec<usize> = vec![0, 1, 2];
        let signer_sks: Vec<[u8; 32]> = signers.iter().map(|&i| sks[i]).collect();
        let meta = produce_signed_snapshot(
            5_000,
            state_root,
            &chunks,
            4,
            &signers,
            &signer_sks,
        )
        .expect("producer must succeed");

        // (1) chunk_root binds the chunk_roots vector by construction.
        assert_eq!(meta.chunk_root, crate::merkle::merkle_root(&meta.chunk_roots));
        // (2) BLS quorum signature verifies under the canonical digest.
        verify_manifest_signature(
            meta.pivot_height,
            meta.state_root,
            meta.chunk_root,
            meta.chunk_roots.len() as u64,
            &meta.bls_quorum_sig,
            &meta.bls_signers,
            &validator_keys,
            3,
        )
        .expect("consumer must accept producer manifest");
        // (3) Per-chunk inclusion proof against chunk_root.
        let proofs = build_inclusion_proofs(&meta.chunk_roots);
        for (i, root) in meta.chunk_roots.iter().enumerate() {
            verify_chunk_inclusion(i as u64, *root, &proofs[i], meta.chunk_root)
                .expect("inclusion proof must verify");
        }
        // (4) Production constructor accepts the same committee shape.
        let _coord = SyncCoordinator::new(
            Arc::new(DummyPeer),
            validator_keys.clone(),
            3,
        );
    }

    struct DummyPeer;
    #[async_trait::async_trait]
    impl crate::coordinator::SyncPeer for DummyPeer {
        async fn tip(&self) -> Result<(u64, H256), SyncError> {
            Ok((0, H256::zero()))
        }
        async fn get_headers(
            &self,
            _: u64,
            _: u32,
        ) -> Result<Vec<zbx_types::block::BlockHeader>, SyncError> {
            Ok(vec![])
        }
        async fn get_snapshot_meta(
            &self,
            _: u64,
        ) -> Result<crate::coordinator::SnapshotMeta, SyncError> {
            Err(SyncError::Interrupted("not used".into()))
        }
        async fn get_chunk(
            &self,
            _: u64,
            _: u64,
        ) -> Result<Vec<(H256, Vec<u8>)>, SyncError> {
            Ok(vec![])
        }
    }

    /// Hard-enforce: the production constructor MUST panic on an
    /// empty validator set in BOTH debug and release (the v3 fix
    /// upgraded `debug_assert!` → `assert!`). This test is the
    /// runtime witness that the fail-open path the architect flagged
    /// is gone.
    #[test]
    #[should_panic(expected = "non-empty validator set")]
    fn production_constructor_panics_on_empty_validator_set() {
        let _ = SyncCoordinator::new(Arc::new(DummyPeer), Vec::new(), 0);
    }

    /// v4 architect-review follow-up: `with_validator_set` must
    /// mirror the constructor's hard-enforced invariant. Otherwise
    /// an operator could construct safely then re-open the fail-
    /// open path by passing `vec![], 0` here.
    #[test]
    #[should_panic(expected = "non-empty validator set")]
    fn with_validator_set_panics_on_empty_set() {
        let sks: Vec<[u8; 32]> = (0..3u8).map(sk_from).collect();
        let pks: Vec<BlsPubKey> = sks.iter().map(pk_from).collect();
        let validator_keys: Vec<(Address, BlsPubKey)> = pks
            .iter()
            .enumerate()
            .map(|(i, pk)| (Address([i as u8; 20]), pk.clone()))
            .collect();
        let coord = SyncCoordinator::new(Arc::new(DummyPeer), validator_keys, 2);
        // Attempt to clear committee via builder — must panic.
        let _ = coord.with_validator_set(Vec::new(), 0);
    }

    /// v4 architect-review follow-up: quorum > committee size is
    /// equally a configuration bug.
    #[test]
    #[should_panic(expected = "non-empty validator set")]
    fn with_validator_set_panics_on_quorum_gt_committee() {
        let sks: Vec<[u8; 32]> = (0..3u8).map(sk_from).collect();
        let pks: Vec<BlsPubKey> = sks.iter().map(pk_from).collect();
        let validator_keys: Vec<(Address, BlsPubKey)> = pks
            .iter()
            .enumerate()
            .map(|(i, pk)| (Address([i as u8; 20]), pk.clone()))
            .collect();
        let coord = SyncCoordinator::new(Arc::new(DummyPeer), validator_keys.clone(), 2);
        let _ = coord.with_validator_set(validator_keys, 99);
    }
}
