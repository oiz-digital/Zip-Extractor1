//! BLS-quorum-signed snapshot manifest.
//!
//! Canonical digest:
//! ```text
//! keccak256(
//!   b"ZBX_SNAPSHOT_MANIFEST_V1"  // domain tag
//!   || pivot_height.to_be_bytes()
//!   || state_root.as_bytes()
//!   || chunk_root.as_bytes()      // merkle_root(chunk_roots)
//!   || (chunk_roots.len() as u64).to_be_bytes())
//! ```
//! `chunk_root` already binds the chunk_roots vector, so individual
//! roots need not appear in the digest.

use crate::error::SyncError;
use crate::merkle::{merkle_proof, merkle_root, verify_proof, MerkleProof};
use zbx_crypto::keccak::keccak256;
use zbx_threshold::{
    bls_aggregate, bls_fast_agg_verify, bls_sign, BlsAggSignature, BlsPubKey,
    BlsSignature, ValidatorBitmap,
};
use zbx_types::{address::Address, H256};

/// Domain separation tag for the canonical manifest digest. Bumped on
/// any change to the digest layout.
pub const MANIFEST_DOMAIN_TAG: &[u8] = b"ZBX_SNAPSHOT_MANIFEST_V1";

/// Compute the canonical manifest digest that the BLS quorum signs.
///
/// `chunk_root` is the Merkle root over `chunk_roots`; the caller is
/// responsible for ensuring `chunk_root == merkle_root(chunk_roots)`
/// (this function does NOT recompute it — it MUST be the same value
/// the manifest will carry, otherwise verification fails).
pub fn manifest_digest(
    pivot_height: u64,
    state_root: H256,
    chunk_root: H256,
    chunk_count: u64,
) -> H256 {
    let mut buf = Vec::with_capacity(MANIFEST_DOMAIN_TAG.len() + 8 + 32 + 32 + 8);
    buf.extend_from_slice(MANIFEST_DOMAIN_TAG);
    buf.extend_from_slice(&pivot_height.to_be_bytes());
    buf.extend_from_slice(state_root.as_bytes());
    buf.extend_from_slice(chunk_root.as_bytes());
    buf.extend_from_slice(&chunk_count.to_be_bytes());
    H256::from(keccak256(&buf))
}

/// Producer-side helper: given the chunk-root list and the BLS secret
/// keys of the signing validators (in the order they appear in the
/// committee), build the manifest's `chunk_root`, BLS-aggregate
/// signature, and signer bitmap.
///
/// `signer_indices` are the positions of the signing validators in
/// the canonical committee order — those bits are set in the
/// returned `ValidatorBitmap` and the corresponding secret keys MUST
/// be supplied in the same order in `signer_secret_keys`.
///
/// Caller is responsible for ensuring `signer_indices.len() >= quorum`
/// — this helper signs whatever it is given.
pub fn build_signed_manifest(
    pivot_height: u64,
    state_root: H256,
    chunk_roots: &[H256],
    n_validators: usize,
    signer_indices: &[usize],
    signer_secret_keys: &[[u8; 32]],
) -> Result<SignedManifest, SyncError> {
    if chunk_roots.is_empty() {
        return Err(SyncError::Interrupted("cannot sign empty chunk_roots".into()));
    }
    if signer_indices.len() != signer_secret_keys.len() {
        return Err(SyncError::Interrupted(format!(
            "build_signed_manifest: {} indices vs {} keys",
            signer_indices.len(),
            signer_secret_keys.len()
        )));
    }
    let chunk_root = merkle_root(chunk_roots);
    let digest = manifest_digest(
        pivot_height,
        state_root,
        chunk_root,
        chunk_roots.len() as u64,
    );
    // BLS-sign the digest with each signer key, aggregate into one sig.
    let sigs: Vec<BlsSignature> = signer_secret_keys
        .iter()
        .map(|sk| bls_sign(sk, digest.as_bytes()))
        .collect();
    let agg = bls_aggregate(&sigs).map_err(|e| {
        SyncError::BadManifestSignature(format!("aggregation failed: {e}"))
    })?;
    let mut bitmap = ValidatorBitmap::new(n_validators);
    for &idx in signer_indices {
        if idx >= n_validators {
            return Err(SyncError::Interrupted(format!(
                "signer index {idx} out of range (n={n_validators})"
            )));
        }
        bitmap.set(idx);
    }
    Ok(SignedManifest {
        chunk_root,
        bls_quorum_sig: agg,
        bls_signers: bitmap,
    })
}

/// The producer-built outputs that bind a manifest. `SnapshotMeta`
/// embeds these fields directly; this struct is just a return-shape
/// for `build_signed_manifest`.
#[derive(Debug, Clone)]
pub struct SignedManifest {
    pub chunk_root:     H256,
    pub bls_quorum_sig: BlsAggSignature,
    pub bls_signers:    ValidatorBitmap,
}

/// Consumer-side: verify the manifest's BLS quorum signature against
/// the registered validator set. MUST be called BEFORE any chunk is
/// fetched — accepting a chunk under an unverified manifest defeats
/// the entire binding (the malicious peer would just forge the
/// manifest to commit to its tampered chunks).
pub fn verify_manifest_signature(
    pivot_height: u64,
    state_root: H256,
    chunk_root: H256,
    chunk_count: u64,
    bls_quorum_sig: &BlsAggSignature,
    bls_signers: &ValidatorBitmap,
    validator_keys: &[(Address, BlsPubKey)],
    quorum: usize,
) -> Result<(), SyncError> {
    if validator_keys.is_empty() {
        return Err(SyncError::BadManifestSignature(
            "validator key set empty — cannot verify".into(),
        ));
    }
    if bls_signers.n_validators != validator_keys.len() {
        return Err(SyncError::BadManifestSignature(format!(
            "bitmap size {} != validator set size {}",
            bls_signers.n_validators,
            validator_keys.len()
        )));
    }
    let signed_indices = bls_signers.signed_indices();
    if signed_indices.len() < quorum {
        return Err(SyncError::BadManifestSignature(format!(
            "{} signers < quorum {}",
            signed_indices.len(),
            quorum
        )));
    }
    let mut signing_pks: Vec<BlsPubKey> = Vec::with_capacity(signed_indices.len());
    for i in &signed_indices {
        let (_, pk) = validator_keys.get(*i).ok_or_else(|| {
            SyncError::BadManifestSignature(format!(
                "signer index {i} out of range (n={})",
                validator_keys.len()
            ))
        })?;
        signing_pks.push(pk.clone());
    }
    let digest = manifest_digest(pivot_height, state_root, chunk_root, chunk_count);
    bls_fast_agg_verify(&signing_pks, digest.as_bytes(), bls_quorum_sig)
        .map_err(|e| SyncError::BadManifestSignature(format!("pairing failed: {e}")))?;
    Ok(())
}

/// Consumer-side: prove that a single chunk's committed root
/// (`chunk_roots[chunk_id]`) is included in the manifest's
/// `chunk_root` Merkle commitment.
///
/// Called for each chunk on receipt — a tampered chunk_root from a
/// malicious peer is rejected immediately without first downloading
/// every other chunk and rebuilding the global state trie.
pub fn verify_chunk_inclusion(
    chunk_id: u64,
    chunk_root_at_id: H256,
    proof: &MerkleProof,
    manifest_chunk_root: H256,
) -> Result<(), SyncError> {
    if !verify_proof(
        chunk_root_at_id,
        chunk_id as usize,
        proof,
        manifest_chunk_root,
    ) {
        return Err(SyncError::ChunkRootMismatch { chunk: chunk_id });
    }
    Ok(())
}

/// Helper for producers to pre-compute every chunk's inclusion proof.
/// Returns a `Vec` aligned with `chunk_roots` (proof at index `i`
/// proves inclusion of chunk_roots\[i\]).
pub fn build_inclusion_proofs(chunk_roots: &[H256]) -> Vec<MerkleProof> {
    (0..chunk_roots.len())
        .map(|i| {
            merkle_proof(chunk_roots, i)
                .expect("index in 0..len() always yields a proof")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbx_threshold::bls_aggregate::BlsPubKey as TBlsPubKey;
    use zbx_crypto::bls as ckbls;

    fn deterministic_sk(seed: u8) -> [u8; 32] {
        let mut k = [0u8; 32];
        for (i, b) in k.iter_mut().enumerate() {
            *b = seed.wrapping_add(i as u8 + 1);
        }
        k
    }

    fn pk_from_sk(sk: &[u8; 32]) -> TBlsPubKey {
        let p = ckbls::BlsPrivKey::from_bytes(sk).unwrap();
        TBlsPubKey(*p.to_pubkey().as_bytes())
    }

    fn h(b: u8) -> H256 {
        let mut x = [0u8; 32];
        x[31] = b;
        H256(x)
    }

    /// Round-trip: producer signs, consumer verifies — both happy paths.
    #[test]
    fn signed_manifest_roundtrip_quorum_3of4() {
        let n = 4;
        let sks: Vec<_> = (0..n as u8).map(deterministic_sk).collect();
        let pks: Vec<_> = sks.iter().map(pk_from_sk).collect();
        let validator_keys: Vec<(Address, TBlsPubKey)> = pks
            .iter()
            .enumerate()
            .map(|(i, pk)| (Address([i as u8; 20]), pk.clone()))
            .collect();

        let chunk_roots: Vec<H256> = (1..=8u8).map(h).collect();
        let pivot_height = 1234;
        let state_root = h(0xAA);

        // Sign with validators {0, 1, 2} (3-of-4 quorum).
        let signer_indices = [0usize, 1, 2];
        let signer_sks: Vec<_> = signer_indices.iter().map(|i| sks[*i]).collect();
        let signed = build_signed_manifest(
            pivot_height,
            state_root,
            &chunk_roots,
            n,
            &signer_indices,
            &signer_sks,
        )
        .expect("build_signed_manifest");

        // Verify
        verify_manifest_signature(
            pivot_height,
            state_root,
            signed.chunk_root,
            chunk_roots.len() as u64,
            &signed.bls_quorum_sig,
            &signed.bls_signers,
            &validator_keys,
            3, // quorum
        )
        .expect("manifest must verify");

        // Per-chunk inclusion proofs round-trip.
        let proofs = build_inclusion_proofs(&chunk_roots);
        for (i, root) in chunk_roots.iter().enumerate() {
            verify_chunk_inclusion(i as u64, *root, &proofs[i], signed.chunk_root)
                .expect("chunk inclusion must verify");
        }
    }

    /// Negative: tampered chunk_root → inclusion proof fails.
    #[test]
    fn tampered_chunk_root_inclusion_rejected() {
        let chunk_roots: Vec<H256> = (1..=4u8).map(h).collect();
        let root = merkle_root(&chunk_roots);
        let proofs = build_inclusion_proofs(&chunk_roots);
        let res = verify_chunk_inclusion(2, h(0xFF), &proofs[2], root);
        assert!(matches!(res, Err(SyncError::ChunkRootMismatch { chunk: 2 })));
    }

    /// Negative: digest mismatch → BLS sig verification fails.
    #[test]
    fn manifest_signature_rejects_digest_tamper() {
        let n = 3;
        let sks: Vec<_> = (10..(10 + n as u8)).map(deterministic_sk).collect();
        let pks: Vec<_> = sks.iter().map(pk_from_sk).collect();
        let validator_keys: Vec<(Address, TBlsPubKey)> = pks
            .iter()
            .enumerate()
            .map(|(i, pk)| (Address([i as u8 + 10; 20]), pk.clone()))
            .collect();

        let chunk_roots: Vec<H256> = (1..=4u8).map(h).collect();
        let signer_indices = [0usize, 1, 2];
        let signer_sks: Vec<_> = signer_indices.iter().map(|i| sks[*i]).collect();
        let signed = build_signed_manifest(99, h(1), &chunk_roots, n, &signer_indices, &signer_sks)
            .unwrap();

        // Tamper with state_root in the verifier's view → digest changes →
        // pairing must fail.
        let res = verify_manifest_signature(
            99,
            h(2), // tampered
            signed.chunk_root,
            chunk_roots.len() as u64,
            &signed.bls_quorum_sig,
            &signed.bls_signers,
            &validator_keys,
            3,
        );
        assert!(matches!(res, Err(SyncError::BadManifestSignature(_))));
    }

    /// Negative: bitmap claims fewer signers than quorum.
    #[test]
    fn manifest_signature_rejects_below_quorum() {
        let n = 4;
        let sks: Vec<_> = (20..(20 + n as u8)).map(deterministic_sk).collect();
        let pks: Vec<_> = sks.iter().map(pk_from_sk).collect();
        let validator_keys: Vec<(Address, TBlsPubKey)> = pks
            .iter()
            .enumerate()
            .map(|(i, pk)| (Address([i as u8 + 20; 20]), pk.clone()))
            .collect();
        let chunk_roots: Vec<H256> = (1..=4u8).map(h).collect();
        let signer_indices = [0usize, 1]; // only 2 signers
        let signer_sks: Vec<_> = signer_indices.iter().map(|i| sks[*i]).collect();
        let signed = build_signed_manifest(7, h(3), &chunk_roots, n, &signer_indices, &signer_sks)
            .unwrap();
        // quorum = 3 → must reject.
        let res = verify_manifest_signature(
            7,
            h(3),
            signed.chunk_root,
            chunk_roots.len() as u64,
            &signed.bls_quorum_sig,
            &signed.bls_signers,
            &validator_keys,
            3,
        );
        assert!(matches!(res, Err(SyncError::BadManifestSignature(_))));
    }
}
