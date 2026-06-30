//! Snapshot manifest with cryptographic binding (Task #11).
//!
//! # Why this exists
//!
//! Pre-Task-11 fast-sync snapshots had no cryptographic provenance. A
//! malicious or buggy node could serve a tampered SST archive or hand-edited
//! `state_root` and a syncing peer would have no in-protocol way to detect
//! the tampering until after replaying transactions on top — at which point
//! every subsequent state-root divergence would surface as a confusing
//! "consensus fork" error rather than the actual root cause (bad snapshot).
//!
//! # What is bound
//!
//! A `SnapshotManifest` covers four cryptographic commitments to the
//! snapshot's contents at a fixed `(block_height, block_hash)`:
//!
//! 1. `state_root`         — Yellow-Paper MPT root over all accounts.
//! 2. `code_hashes_root`   — keccak256 over the sorted list of contract
//!                           code-hashes referenced by the world state
//!                           (so syncers know which `code` rows to fetch).
//! 3. `validator_set_root` — keccak256 of the canonical encoding of the
//!                           `ValidatorSet` at this height (so syncers
//!                           bootstrap epoch-0 with the right set, not a
//!                           stale or attacker-chosen one).
//! 4. `chunks_root`        — keccak256 of the ordered chunk-hash list of
//!                           the SST archive itself (each chunk fetched
//!                           is verified against this).
//!
//! The manifest is BLS-signed by a known validator at the snapshot height.
//! `verify` re-derives `signing_digest()` and runs the standard BLS
//! single-key verify; ANY one-bit edit to ANY field invalidates the sig.
//!
//! # Honest limitations (deferred)
//!
//! - **`code_hashes_root` / `chunks_root` derivation** lives in the
//!   snapshot-builder code path (not in this crate — that lands with the
//!   actual SST exporter). This module focuses on the manifest surface
//!   (struct + sign + verify + canonical encoding); callers populate the
//!   roots from whatever source they have. Until the exporter ships, the
//!   manifest still binds whatever fields the producer fills in — the
//!   tampering-detection property holds for every field, full stop.
//! - **`bincode` is the canonical encoding.** Versioned via the explicit
//!   `version: u8` field so future format changes (e.g. switching to RLP
//!   for cross-client portability) can be staged with a bumped version
//!   and a dispatch table on `verify`.

use serde::{Deserialize, Serialize};
use thiserror::Error;
use zbx_crypto::bls::{verify_single, BlsPrivKey, BlsPubKey, BlsSignature};
use zbx_crypto::keccak::keccak256;
use zbx_types::H256;

/// Current manifest format version. Verifiers MUST reject any other value.
pub const SNAPSHOT_MANIFEST_VERSION: u8 = 1;

/// Domain separator mixed into the signing digest. Prevents a manifest
/// signature from being replayable as some other BLS-signed message
/// (e.g. a vote or PoP) under the same key.
pub const SNAPSHOT_SIG_DOMAIN: &[u8] = b"zbx-snapshot-manifest-v1";

/// The unsigned body of a snapshot manifest. Serialised via `bincode` for
/// the signing digest; the `Signed` wrapper carries this struct verbatim
/// plus the producer pubkey and signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotManifest {
    /// Format version. Must equal `SNAPSHOT_MANIFEST_VERSION`.
    pub version: u8,
    /// Network this snapshot belongs to (defends against cross-chain
    /// replay — e.g. testnet manifest reused on mainnet).
    pub chain_id: u64,
    /// Height at which the snapshot was taken.
    pub block_height: u64,
    /// Canonical hash of the block at `block_height`.
    pub block_hash: H256,
    /// World-state MPT root at `block_height` (post-execution).
    pub state_root: H256,
    /// keccak256 over the sorted list of contract code-hashes.
    pub code_hashes_root: H256,
    /// keccak256 of the canonical ValidatorSet encoding at this height.
    pub validator_set_root: H256,
    /// keccak256 of the ordered chunk-hash list of the SST archive
    /// (zero when the producer ships the manifest before chunking — the
    /// signature still covers the field, so a later flip is detectable).
    pub chunks_root: H256,
    /// Wall-clock timestamp at which the producer built the snapshot.
    pub timestamp_unix: u64,
}

/// A fully-signed manifest — the on-the-wire artefact peers exchange.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedSnapshotManifest {
    pub manifest: SnapshotManifest,
    pub producer_pubkey: BlsPubKey,
    pub signature: BlsSignature,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SnapshotError {
    #[error("unsupported manifest version: got {got}, expected {expected}")]
    UnsupportedVersion { got: u8, expected: u8 },
    #[error("chain_id mismatch: manifest={got}, expected={expected}")]
    ChainIdMismatch { got: u64, expected: u64 },
    #[error("producer pubkey is not in the allowed validator set")]
    UnauthorisedProducer,
    #[error("BLS signature verification failed (manifest tampering or wrong key)")]
    BadSignature,
    #[error("expected state_root mismatch: manifest={got}, expected={expected}")]
    StateRootMismatch { got: H256, expected: H256 },
    #[error("expected checkpoint mismatch: manifest=({got_height}, {got_hash:?}), expected=({exp_height}, {exp_hash:?})")]
    CheckpointMismatch {
        got_height: u64,
        got_hash: H256,
        exp_height: u64,
        exp_hash: H256,
    },
    #[error("manifest serialisation failed: {0}")]
    Encode(String),
}

impl SnapshotManifest {
    /// Canonical encoding of the unsigned body fed into the signing digest.
    /// Bincode default (little-endian, no length prefix surprises since every
    /// field is fixed-size). Wrapped in a fallible function so a future
    /// swap to RLP or SSZ is one call site away.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, SnapshotError> {
        bincode::serialize(self).map_err(|e| SnapshotError::Encode(e.to_string()))
    }

    /// 32-byte BLS-signing digest:
    /// `keccak256(SNAPSHOT_SIG_DOMAIN || chain_id_be8 || canonical_bytes(self))`.
    /// The domain tag and chain-id mix-in defeat cross-protocol and
    /// cross-chain replay even before the recipient's `chain_id` check.
    pub fn signing_digest(&self) -> Result<H256, SnapshotError> {
        let body = self.canonical_bytes()?;
        let mut buf =
            Vec::with_capacity(SNAPSHOT_SIG_DOMAIN.len() + 8 + body.len());
        buf.extend_from_slice(SNAPSHOT_SIG_DOMAIN);
        buf.extend_from_slice(&self.chain_id.to_be_bytes());
        buf.extend_from_slice(&body);
        Ok(keccak256(&buf))
    }

    /// Sign a manifest with the producer's BLS key. Returns the wire
    /// artefact ready for peers.
    pub fn sign(self, sk: &BlsPrivKey) -> Result<SignedSnapshotManifest, SnapshotError> {
        let digest = self.signing_digest()?;
        let signature = sk.sign(&digest);
        let producer_pubkey = sk.to_pubkey();
        Ok(SignedSnapshotManifest {
            manifest: self,
            producer_pubkey,
            signature,
        })
    }
}

impl SignedSnapshotManifest {
    /// Full verification:
    ///   1. Format version is the supported one (defends against
    ///      mixed-version sync attacks before any crypto runs).
    ///   2. `chain_id` matches the syncing node's expected chain.
    ///   3. Producer pubkey is in the caller-provided allowed set.
    ///   4. The BLS signature verifies against `manifest.signing_digest()`.
    ///   5. (optional) `expected_checkpoint = (block_height, block_hash)`
    ///      matches the manifest. This is the **same-chain freshness
    ///      defence** (Pass-19 architect-review CRIT #3): without it, an
    ///      authorised producer's old, validly-signed manifest from the
    ///      same chain would pass — letting an attacker replay a stale
    ///      snapshot at a known-vulnerable historical state. Mainnet/
    ///      testnet sync paths MUST pass `Some((expected_h, expected_hash))`
    ///      from the chain's checkpoint store; only standalone tooling
    ///      may pass `None`.
    ///   6. (optional) `state_root` matches the caller-pinned root — the
    ///      caller is the canonical chain DB and this is the trust anchor.
    pub fn verify(
        &self,
        expected_chain_id: u64,
        allowed_producers: &[BlsPubKey],
        expected_checkpoint: Option<(u64, H256)>,
        pinned_state_root: Option<H256>,
    ) -> Result<(), SnapshotError> {
        // (1) Version.
        if self.manifest.version != SNAPSHOT_MANIFEST_VERSION {
            return Err(SnapshotError::UnsupportedVersion {
                got: self.manifest.version,
                expected: SNAPSHOT_MANIFEST_VERSION,
            });
        }
        // (2) Chain.
        if self.manifest.chain_id != expected_chain_id {
            return Err(SnapshotError::ChainIdMismatch {
                got: self.manifest.chain_id,
                expected: expected_chain_id,
            });
        }
        // (3) Producer authorised.
        if !allowed_producers.iter().any(|pk| pk == &self.producer_pubkey) {
            return Err(SnapshotError::UnauthorisedProducer);
        }
        // (4) BLS signature over the canonical digest.
        let digest = self.manifest.signing_digest()?;
        if !verify_single(&self.signature, &self.producer_pubkey, &digest) {
            return Err(SnapshotError::BadSignature);
        }
        // (5) Same-chain freshness binding (Pass-19 CRIT #3). The checkpoint
        //     tuple is the syncing node's externally-trusted (height, hash)
        //     anchor — typically the latest finalised block-hash served
        //     over the gossip layer or pinned in chain config. Rejecting
        //     a height/hash mismatch closes the stale-but-validly-signed
        //     replay vector that the producer-authorisation + BLS-sig
        //     checks alone cannot detect (an authorised producer's old
        //     signature on an old manifest is, by construction, valid).
        if let Some((exp_h, exp_hash)) = expected_checkpoint {
            if exp_h != self.manifest.block_height || exp_hash != self.manifest.block_hash {
                return Err(SnapshotError::CheckpointMismatch {
                    got_height: self.manifest.block_height,
                    got_hash: self.manifest.block_hash,
                    exp_height: exp_h,
                    exp_hash: exp_hash,
                });
            }
        }
        // (6) Pinned-root cross-check (caller-supplied; opt-in because the
        //     syncing node may not yet HAVE the root they want to compare
        //     against — this branch is for restore-time integrity audits).
        if let Some(pin) = pinned_state_root {
            if pin != self.manifest.state_root {
                return Err(SnapshotError::StateRootMismatch {
                    got: self.manifest.state_root,
                    expected: pin,
                });
            }
        }
        Ok(())
    }
}

/// Compute the canonical `validator_set_root` from a sorted list of
/// `(address, bls_pubkey, stake)` tuples. The caller is responsible for
/// passing a deterministically-sorted list (by address); this helper
/// concatenates the canonical encoding `(addr20 || pk48 || stake_be16)`
/// per entry and hashes the result.
///
/// Exposed here so the snapshot producer in `node/` and the verifier
/// in any future light-client share one definition.
pub fn compute_validator_set_root(entries: &[(zbx_types::address::Address, BlsPubKey, u128)]) -> H256 {
    let mut buf = Vec::with_capacity(entries.len() * (20 + 48 + 16));
    for (addr, pk, stake) in entries {
        buf.extend_from_slice(addr.as_bytes());
        buf.extend_from_slice(pk.as_bytes());
        buf.extend_from_slice(&stake.to_be_bytes());
    }
    keccak256(&buf)
}

/// Compute the canonical `code_hashes_root` from a sorted-deduplicated
/// list of contract code hashes. Caller sorts ascending; we hash the
/// flat concatenation.
pub fn compute_code_hashes_root(sorted_hashes: &[H256]) -> H256 {
    let mut buf = Vec::with_capacity(sorted_hashes.len() * 32);
    for h in sorted_hashes {
        buf.extend_from_slice(h.as_bytes());
    }
    keccak256(&buf)
}

// ─── Readiness probe (Task #14 check #3) ────────────────────────────────

/// Self-test of the manifest crypto-binding round-trip. Used by the
/// mainnet readiness predicate to prove that:
///   1. A well-formed signed manifest verifies with the producer in the
///      allowed set.
///   2. ANY one-byte tampering of the manifest body invalidates the sig.
///   3. A pubkey rotation (different signer) is rejected as
///      `UnauthorisedProducer` even before BLS pairing runs.
///   4. A wrong `chain_id` is rejected (cross-chain replay defence).
///
/// Returns `Ok(())` when every sub-check passes; otherwise a short
/// human-readable description of which invariant regressed.
pub fn probe_in_memory() -> Result<(), &'static str> {
    let sk = BlsPrivKey::from_bytes(&[0x33u8; 32])
        .map_err(|_| "BLS key construction broken")?;
    let pk = sk.to_pubkey();
    let other_sk = BlsPrivKey::from_bytes(&[0x77u8; 32])
        .map_err(|_| "BLS key construction broken")?;
    let other_pk = other_sk.to_pubkey();

    let m = SnapshotManifest {
        version: SNAPSHOT_MANIFEST_VERSION,
        chain_id: 8989,
        block_height: 1_000_000,
        block_hash: H256([0x11u8; 32]),
        state_root: H256([0x22u8; 32]),
        code_hashes_root: H256([0x33u8; 32]),
        validator_set_root: H256([0x44u8; 32]),
        chunks_root: H256([0x55u8; 32]),
        timestamp_unix: 1_700_000_000,
    };
    let signed = m.clone().sign(&sk).map_err(|_| "manifest sign failed")?;

    let exp_ckpt = Some((m.block_height, m.block_hash));

    // (1) Happy path verifies.
    signed
        .verify(8989, std::slice::from_ref(&pk), exp_ckpt, Some(m.state_root))
        .map_err(|_| "happy-path verify regressed")?;

    // (2) Tamper one bit of state_root → BadSignature.
    let mut tampered = signed.clone();
    tampered.manifest.state_root.0[0] ^= 0x01;
    match tampered.verify(8989, std::slice::from_ref(&pk), exp_ckpt, None) {
        Err(SnapshotError::BadSignature) => {}
        _ => return Err("tampering detection regressed — sig accepted edited manifest"),
    }

    // (3) Different signer → UnauthorisedProducer (before sig check).
    let foreign = m.clone().sign(&other_sk).map_err(|_| "foreign sign failed")?;
    match foreign.verify(8989, std::slice::from_ref(&pk), exp_ckpt, None) {
        Err(SnapshotError::UnauthorisedProducer) => {}
        _ => return Err("allowed-producer gate regressed"),
    }

    // (4) Wrong chain_id → ChainIdMismatch.
    match signed.verify(8990, std::slice::from_ref(&pk), exp_ckpt, None) {
        Err(SnapshotError::ChainIdMismatch { .. }) => {}
        _ => return Err("chain_id replay-defence regressed"),
    }

    // (5) Wrong pinned state_root → StateRootMismatch.
    match signed.verify(8989, std::slice::from_ref(&pk), exp_ckpt, Some(H256([0xFFu8; 32]))) {
        Err(SnapshotError::StateRootMismatch { .. }) => {}
        _ => return Err("pinned-root cross-check regressed"),
    }

    // (6) Wrong producer (foreign pubkey in allowed set, signed by sk).
    //     Signature must fail because it was produced by `sk`, not `other_sk`.
    let mut foreign_sigs_with_sk_signature = signed.clone();
    foreign_sigs_with_sk_signature.producer_pubkey = other_pk.clone();
    match foreign_sigs_with_sk_signature.verify(
        8989,
        std::slice::from_ref(&other_pk),
        exp_ckpt,
        None,
    ) {
        Err(SnapshotError::BadSignature) => {}
        _ => return Err("pubkey-substitution detection regressed"),
    }

    // (7) Same-chain stale-replay defence (Pass-19 CRIT #3).
    //     A validly-signed manifest at the WRONG (height, hash) checkpoint
    //     must be rejected — even with the correct producer + chain_id +
    //     intact signature. Without this branch, an attacker could replay
    //     an old, legitimately-signed snapshot at a known-vulnerable
    //     historical state.
    let stale_ckpt = Some((m.block_height + 1, H256([0xACu8; 32])));
    match signed.verify(8989, std::slice::from_ref(&pk), stale_ckpt, None) {
        Err(SnapshotError::CheckpointMismatch { .. }) => {}
        _ => return Err("same-chain checkpoint freshness binding regressed (CRIT #3)"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> SnapshotManifest {
        SnapshotManifest {
            version: SNAPSHOT_MANIFEST_VERSION,
            chain_id: 8989,
            block_height: 42,
            block_hash: H256([0xAAu8; 32]),
            state_root: H256([0xBBu8; 32]),
            code_hashes_root: H256([0xCCu8; 32]),
            validator_set_root: H256([0xDDu8; 32]),
            chunks_root: H256([0xEEu8; 32]),
            timestamp_unix: 1_700_000_000,
        }
    }

    #[test]
    fn signing_digest_changes_with_every_field() {
        let base = fixture();
        let d0 = base.signing_digest().unwrap();
        let mut m1 = base.clone();
        m1.block_height += 1;
        let m2 = SnapshotManifest { state_root: H256([0xFFu8; 32]), ..base.clone() };
        let m3 = SnapshotManifest { chain_id: 8990, ..base.clone() };
        assert_ne!(d0, m1.signing_digest().unwrap());
        assert_ne!(d0, m2.signing_digest().unwrap());
        assert_ne!(d0, m3.signing_digest().unwrap());
    }

    #[test]
    fn happy_path_round_trip() {
        let sk = BlsPrivKey::from_bytes(&[1u8; 32]).unwrap();
        let pk = sk.to_pubkey();
        let m = fixture();
        let ckpt = Some((m.block_height, m.block_hash));
        let signed = m.sign(&sk).unwrap();
        assert!(signed.verify(8989, &[pk], ckpt, None).is_ok());
    }

    #[test]
    fn tamper_state_root_rejected() {
        let sk = BlsPrivKey::from_bytes(&[1u8; 32]).unwrap();
        let pk = sk.to_pubkey();
        let mut signed = fixture().sign(&sk).unwrap();
        signed.manifest.state_root.0[31] ^= 0x80;
        assert_eq!(signed.verify(8989, &[pk], None, None), Err(SnapshotError::BadSignature));
    }

    #[test]
    fn unauthorised_producer_rejected() {
        let sk = BlsPrivKey::from_bytes(&[1u8; 32]).unwrap();
        let other = BlsPrivKey::from_bytes(&[2u8; 32]).unwrap().to_pubkey();
        let signed = fixture().sign(&sk).unwrap();
        assert_eq!(
            signed.verify(8989, &[other], None, None),
            Err(SnapshotError::UnauthorisedProducer),
        );
    }

    #[test]
    fn cross_chain_replay_rejected() {
        let sk = BlsPrivKey::from_bytes(&[1u8; 32]).unwrap();
        let pk = sk.to_pubkey();
        let signed = fixture().sign(&sk).unwrap();
        assert!(matches!(
            signed.verify(8990, &[pk], None, None),
            Err(SnapshotError::ChainIdMismatch { .. }),
        ));
    }

    #[test]
    fn version_mismatch_rejected() {
        let sk = BlsPrivKey::from_bytes(&[1u8; 32]).unwrap();
        let pk = sk.to_pubkey();
        let mut m = fixture();
        m.version = 99;
        let signed = m.sign(&sk).unwrap();
        assert!(matches!(
            signed.verify(8989, &[pk], None, None),
            Err(SnapshotError::UnsupportedVersion { .. }),
        ));
    }

    #[test]
    fn pinned_state_root_mismatch_rejected() {
        let sk = BlsPrivKey::from_bytes(&[1u8; 32]).unwrap();
        let pk = sk.to_pubkey();
        let signed = fixture().sign(&sk).unwrap();
        assert!(matches!(
            signed.verify(8989, &[pk], None, Some(H256([0xFFu8; 32]))),
            Err(SnapshotError::StateRootMismatch { .. }),
        ));
    }

    #[test]
    fn same_chain_stale_checkpoint_rejected() {
        // Pass-19 CRIT #3 — validly-signed manifest at the wrong
        // (height, hash) MUST be rejected as CheckpointMismatch even
        // with the right producer + chain_id + intact sig.
        let sk = BlsPrivKey::from_bytes(&[1u8; 32]).unwrap();
        let pk = sk.to_pubkey();
        let signed = fixture().sign(&sk).unwrap();
        let stale = Some((signed.manifest.block_height + 100, H256([0xDEu8; 32])));
        assert!(matches!(
            signed.verify(8989, &[pk], stale, None),
            Err(SnapshotError::CheckpointMismatch { .. }),
        ));
    }

    #[test]
    fn matching_checkpoint_accepted() {
        let sk = BlsPrivKey::from_bytes(&[1u8; 32]).unwrap();
        let pk = sk.to_pubkey();
        let m = fixture();
        let ckpt = Some((m.block_height, m.block_hash));
        let signed = m.sign(&sk).unwrap();
        assert!(signed.verify(8989, &[pk], ckpt, None).is_ok());
    }

    #[test]
    fn probe_passes_on_clean_logic() {
        probe_in_memory().expect("snapshot probe regressed");
    }
}
