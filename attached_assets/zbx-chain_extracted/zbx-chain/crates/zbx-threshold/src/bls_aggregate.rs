//! BLS12-381 signature aggregation for ZBX Chain consensus (ZEP-016).
//!
//! Aggregates 2f+1 validator signatures into a single BLS12-381 signature,
//! reducing block-header signature data from O(n) to O(1).
//!
//! ## Scheme: BLS-on-G2 ("min-pubkey-size"), keys on G1
//!
//! ```text
//! G1: 48-byte compressed points (public keys)
//! G2: 96-byte compressed points (signatures)
//!
//! Sign(sk, msg):   σ = sk · H(msg)            where H : {0,1}* → G2 (RFC 9380)
//! Verify(pk, msg, σ): e(g1, σ) == e(pk, H(msg))
//! Aggregate(σ₁..σₙ): σ_agg = Σ σᵢ              (G2 point addition)
//! FastAggVerify([pk₁..pkₙ], msg, σ_agg):
//!   pk_agg = Σ pkᵢ                            (G1 point addition)
//!   e(g1, σ_agg) == e(pk_agg, H(msg))
//! ```
//!
//! ## Pass-17 (SEC-2026-05-09) — real bls12_381
//!
//! Pre-Pass-17 this module shipped a SHA3-based pseudorandom expansion
//! ("hash-to-curve") plus byte-XOR aggregation. Every honest validator's
//! signature was forgeable by an outsider because `bls_verify_single`
//! reconstructed the "expected sig" from the public key alone (treating
//! `pubkey[..32]` as a secret proxy). Wired into HotStuff2 quorum
//! certificates, that meant any production node would have accepted forged
//! QCs on chain 8989. The Pass-12 mitigation was a runtime panic guard
//! (`assert_not_mainnet_bls`) that simply refused to start.
//!
//! Pass-17 replaces the entire body with thin wrappers around
//! [`zbx_crypto::bls`], which uses real bls12_381 G1/G2 arithmetic,
//! the IETF RFC 9380 hash-to-curve `BLS12381G2_XMD:SHA-256_SSWU_RO_`,
//! and a real bilinear pairing check `e(g1, σ) == e(pk, H(msg))`.
//! The mainnet-boot panic guard for this module is removed.
//!
//! Byte layouts (48-byte G1 pubkey, 96-byte G2 sig) are preserved so
//! existing consensus / RPC / serde code keeps working unchanged.

use serde::{Deserialize, Serialize};
use serde_big_array::BigArray;
use thiserror::Error;
use zbx_types::{address::Address, H256};
use zbx_crypto::bls as ckbls;
use zbx_crypto::keccak::keccak256;

// ── Error types ───────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum BLSError {
    #[error("invalid BLS public key: expected 48 bytes, got {0}")]
    InvalidPublicKeyLength(usize),

    #[error("invalid BLS signature: expected 96 bytes, got {0}")]
    InvalidSignatureLength(usize),

    #[error("invalid BLS public key encoding (not a valid G1 point)")]
    InvalidPublicKeyEncoding,

    #[error("invalid BLS signature encoding (not a valid G2 point)")]
    InvalidSignatureEncoding,

    #[error("aggregate verification failed")]
    AggregateVerificationFailed,

    #[error("empty signature set: cannot aggregate zero signatures")]
    EmptySignatureSet,

    #[error("rogue key attack: proof of possession required but missing for {0:?}")]
    MissingProofOfPossession(Address),

    #[error("batch verification failed at index {0}")]
    BatchVerificationFailed(usize),

    #[error("signer bitmap index {idx} out of range (validator set size {n})")]
    SignerIndexOutOfRange { idx: usize, n: usize },
}

// ── BLS Key Types ─────────────────────────────────────────────────────────────

/// BLS12-381 public key (G1 point, 48 bytes compressed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlsPubKey(#[serde(with = "BigArray")] pub [u8; 48]);

/// BLS12-381 signature (G2 point, 96 bytes compressed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlsSignature(#[serde(with = "BigArray")] pub [u8; 96]);

/// BLS12-381 aggregate signature (also a 96-byte G2 point).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlsAggSignature(#[serde(with = "BigArray")] pub [u8; 96]);

/// Proof of possession — prevents rogue-key attacks on aggregate sigs.
/// A PoP is a BLS signature of the validator's ECDSA address with a
/// fixed domain separator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlsProofOfPossession {
    pub validator:  Address,
    pub bls_pubkey: BlsPubKey,
    pub pop:        BlsSignature, // BLS_Sign(bls_sk, keccak256(ecdsa_address || "zbx-bls-pop-v1"))
}

impl BlsPubKey {
    pub fn as_bytes(&self) -> &[u8; 48] { &self.0 }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, BLSError> {
        if bytes.len() != 48 {
            return Err(BLSError::InvalidPublicKeyLength(bytes.len()));
        }
        let mut arr = [0u8; 48];
        arr.copy_from_slice(bytes);
        // Validate the bytes actually decode as a G1 curve point.
        ckbls::BlsPubKey::from_bytes(&arr)
            .map_err(|_| BLSError::InvalidPublicKeyEncoding)?;
        Ok(BlsPubKey(arr))
    }

    /// Verify the proof of possession for this key.
    pub fn verify_pop(&self, pop: &BlsSignature, validator: &Address) -> bool {
        let mut msg = Vec::with_capacity(20 + 14);
        msg.extend_from_slice(&validator.0);
        msg.extend_from_slice(b"zbx-bls-pop-v1");
        bls_verify_single(self, &msg, pop)
    }
}

impl BlsSignature {
    pub fn as_bytes(&self) -> &[u8; 96] { &self.0 }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, BLSError> {
        if bytes.len() != 96 {
            return Err(BLSError::InvalidSignatureLength(bytes.len()));
        }
        let mut arr = [0u8; 96];
        arr.copy_from_slice(bytes);
        // Validate the bytes actually decode as a G2 curve point.
        ckbls::BlsSignature::from_bytes(&arr)
            .map_err(|_| BLSError::InvalidSignatureEncoding)?;
        Ok(BlsSignature(arr))
    }
}

/// Bitmap tracking which validators signed in a committee.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorBitmap {
    pub bits:         Vec<u8>,
    pub n_validators: usize,
}

impl ValidatorBitmap {
    pub fn new(n_validators: usize) -> Self {
        let n_bytes = (n_validators + 7) / 8;
        ValidatorBitmap { bits: vec![0u8; n_bytes], n_validators }
    }

    pub fn set(&mut self, idx: usize) {
        if idx < self.n_validators {
            self.bits[idx / 8] |= 1 << (idx % 8);
        }
    }

    pub fn is_set(&self, idx: usize) -> bool {
        if idx >= self.n_validators { return false; }
        (self.bits[idx / 8] >> (idx % 8)) & 1 == 1
    }

    pub fn count(&self) -> usize {
        self.bits.iter().map(|b| b.count_ones() as usize).sum()
    }

    pub fn signed_indices(&self) -> Vec<usize> {
        (0..self.n_validators).filter(|&i| self.is_set(i)).collect()
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Hash an arbitrary byte string to a 32-byte H256 message digest. The
/// underlying `zbx_crypto::bls` API operates on H256 (and then runs RFC 9380
/// `hash_to_curve` over those 32 bytes); we keccak256 first so that signing
/// + verifying agree on the same digest regardless of input length.
#[inline]
fn msg_digest(msg: &[u8]) -> H256 {
    H256::from(keccak256(msg))
}

#[inline]
fn to_ck_pub(pk: &BlsPubKey) -> Result<ckbls::BlsPubKey, BLSError> {
    ckbls::BlsPubKey::from_bytes(&pk.0).map_err(|_| BLSError::InvalidPublicKeyEncoding)
}

#[inline]
fn to_ck_sig(sig: &BlsSignature) -> Result<ckbls::BlsSignature, BLSError> {
    ckbls::BlsSignature::from_bytes(&sig.0).map_err(|_| BLSError::InvalidSignatureEncoding)
}

// ── BLS Signing / Verification (real bls12_381) ───────────────────────────────

/// Sign a message with a BLS private key (32-byte little-endian Fr scalar).
/// Returns a 96-byte compressed G2 point.
///
/// Requires the secret key to be a valid non-zero Fr element. A zero key is
/// rejected by [`zbx_crypto::bls::BlsPrivKey::from_bytes`]; if you pass a
/// zero buffer, this function will return a deterministic all-zero signature
/// blob — verification will then fail closed.
pub fn bls_sign(secret_key: &[u8; 32], message: &[u8]) -> BlsSignature {
    let sk = match ckbls::BlsPrivKey::from_bytes(secret_key) {
        Ok(s)  => s,
        Err(_) => return BlsSignature([0u8; 96]),
    };
    let digest = msg_digest(message);
    let sig    = sk.sign(&digest);
    BlsSignature(*sig.as_bytes())
}

/// Verify a single BLS signature: `e(g1, σ) == e(pk, H(msg))`.
pub fn bls_verify_single(pubkey: &BlsPubKey, message: &[u8], sig: &BlsSignature) -> bool {
    let ck_pk  = match to_ck_pub(pubkey) { Ok(p) => p, Err(_) => return false };
    let ck_sig = match to_ck_sig(sig)    { Ok(s) => s, Err(_) => return false };
    let digest = msg_digest(message);
    ckbls::verify_single(&ck_sig, &ck_pk, &digest)
}

// ── BLS Aggregation (real G2 / G1 point addition) ─────────────────────────────

/// Aggregate multiple BLS signatures into one via real G2 point addition.
/// All signatures should be over the same message for the
/// [`bls_fast_agg_verify`] form to apply.
pub fn bls_aggregate(sigs: &[BlsSignature]) -> Result<BlsAggSignature, BLSError> {
    if sigs.is_empty() {
        return Err(BLSError::EmptySignatureSet);
    }
    let ck_sigs: Vec<_> = sigs.iter()
        .map(to_ck_sig)
        .collect::<Result<_, _>>()?;
    let agg = ckbls::aggregate_signatures(&ck_sigs)
        .map_err(|_| BLSError::AggregateVerificationFailed)?;
    Ok(BlsAggSignature(*agg.as_bytes()))
}

/// Aggregate public keys for fast-aggregate verification (real G1 addition).
pub fn bls_aggregate_pubkeys(pubkeys: &[BlsPubKey]) -> Result<BlsPubKey, BLSError> {
    if pubkeys.is_empty() {
        return Err(BLSError::EmptySignatureSet);
    }
    let ck_pks: Vec<_> = pubkeys.iter()
        .map(to_ck_pub)
        .collect::<Result<_, _>>()?;
    let agg = ckbls::aggregate_pubkeys(&ck_pks)
        .map_err(|_| BLSError::AggregateVerificationFailed)?;
    Ok(BlsPubKey(*agg.as_bytes()))
}

/// Fast aggregate verify: one aggregate signature over one message.
/// Performs a real bilinear pairing check; cost is constant in the
/// number of signers (2 pairings).
pub fn bls_fast_agg_verify(
    pubkeys:  &[BlsPubKey],
    message:  &[u8],
    agg_sig:  &BlsAggSignature,
) -> Result<(), BLSError> {
    if pubkeys.is_empty() {
        return Err(BLSError::EmptySignatureSet);
    }
    let ck_pks: Vec<_> = pubkeys.iter()
        .map(to_ck_pub)
        .collect::<Result<_, _>>()?;
    let ck_sig = ckbls::BlsSignature::from_bytes(&agg_sig.0)
        .map_err(|_| BLSError::InvalidSignatureEncoding)?;
    let digest = msg_digest(message);
    if ckbls::verify_aggregate(&ck_sig, &ck_pks, &digest) {
        Ok(())
    } else {
        Err(BLSError::AggregateVerificationFailed)
    }
}

/// Batch-verify multiple `(pubkey, message, signature)` triples.
/// Returns on first failure with its index. Each verification is a real
/// pairing check; this is a sequential fan-out (no Miller-loop batching
/// trick yet — that's a future optimisation).
pub fn bls_batch_verify(
    triples: &[(BlsPubKey, Vec<u8>, BlsSignature)],
) -> Result<(), BLSError> {
    for (idx, (pk, msg, sig)) in triples.iter().enumerate() {
        if !bls_verify_single(pk, msg, sig) {
            return Err(BLSError::BatchVerificationFailed(idx));
        }
    }
    Ok(())
}

// ── Quorum Certificate ────────────────────────────────────────────────────────

/// Quorum Certificate using BLS aggregate signature.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BLSQuorumCertificate {
    /// Block hash being certified.
    pub block_hash:    zbx_types::H256,
    /// Round number.
    pub round:         u64,
    /// BLS aggregate signature from 2f+1 validators.
    pub agg_signature: BlsAggSignature,
    /// Which validators signed (bitmap).
    pub signer_bitmap: ValidatorBitmap,
}

impl BLSQuorumCertificate {
    /// Verify this QC against the registered validator set.
    pub fn verify(
        &self,
        validator_bls_keys: &[(Address, BlsPubKey)],
        quorum: usize,
    ) -> Result<(), BLSError> {
        let signed_indices = self.signer_bitmap.signed_indices();
        // Pass-17 architect-review fix: pre-Pass-17 this used `filter_map`,
        // silently dropping out-of-range indices. A malicious bitmap could
        // therefore claim quorum (indices.len() >= quorum) while the actual
        // signer keyset shrank below quorum after the filter, and
        // `bls_fast_agg_verify` would then run on a small valid subset and
        // accept. Now any out-of-range index hard-fails the QC.
        let mut signing_pks: Vec<BlsPubKey> = Vec::with_capacity(signed_indices.len());
        for &i in &signed_indices {
            let (_, pk) = validator_bls_keys.get(i)
                .ok_or(BLSError::SignerIndexOutOfRange { idx: i, n: validator_bls_keys.len() })?;
            signing_pks.push(pk.clone());
        }
        debug_assert_eq!(signing_pks.len(), signed_indices.len());
        if signing_pks.len() < quorum {
            return Err(BLSError::AggregateVerificationFailed);
        }
        bls_fast_agg_verify(
            &signing_pks,
            &self.block_hash.0,
            &self.agg_signature,
        )
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use zbx_types::H256;

    /// A simple, deterministic, non-zero secret key for tests.
    fn sk(seed: u8) -> [u8; 32] {
        let mut k = [0u8; 32];
        for (i, b) in k.iter_mut().enumerate() {
            *b = seed.wrapping_add(i as u8 + 1);
        }
        k
    }

    fn pk_of(seed: u8) -> BlsPubKey {
        let priv_ = ckbls::BlsPrivKey::from_bytes(&sk(seed)).unwrap();
        BlsPubKey(*priv_.to_pubkey().as_bytes())
    }

    #[test]
    fn bitmap_operations() {
        let mut bm = ValidatorBitmap::new(10);
        bm.set(0); bm.set(3); bm.set(9);
        assert!(bm.is_set(0));
        assert!(bm.is_set(3));
        assert!(bm.is_set(9));
        assert!(!bm.is_set(1));
        assert_eq!(bm.count(), 3);
        assert_eq!(bm.signed_indices(), vec![0, 3, 9]);
    }

    #[test]
    fn sign_and_verify_real_pairing_roundtrip() {
        let secret = sk(7);
        let pk     = pk_of(7);
        let msg    = b"zbx block hash 1".to_vec();
        let sig    = bls_sign(&secret, &msg);
        assert!(bls_verify_single(&pk, &msg, &sig));
    }

    #[test]
    fn verify_rejects_wrong_message() {
        let secret = sk(8);
        let pk     = pk_of(8);
        let sig    = bls_sign(&secret, b"hello");
        assert!(!bls_verify_single(&pk, b"goodbye", &sig));
    }

    #[test]
    fn verify_rejects_wrong_pubkey() {
        let secret = sk(9);
        let other  = pk_of(10);
        let sig    = bls_sign(&secret, b"shared");
        assert!(!bls_verify_single(&other, b"shared", &sig));
    }

    #[test]
    fn aggregate_three_signers_same_message() {
        let seeds = [11u8, 12, 13];
        let secrets: Vec<_> = seeds.iter().map(|&s| sk(s)).collect();
        let pks:     Vec<_> = seeds.iter().map(|&s| pk_of(s)).collect();
        let msg = b"shared block hash".to_vec();
        let sigs: Vec<_> = secrets.iter().map(|s| bls_sign(s, &msg)).collect();
        let agg = bls_aggregate(&sigs).unwrap();
        assert!(bls_fast_agg_verify(&pks, &msg, &agg).is_ok());
    }

    #[test]
    fn aggregate_rejects_one_tampered_signer() {
        let seeds = [21u8, 22, 23];
        let secrets: Vec<_> = seeds.iter().map(|&s| sk(s)).collect();
        let pks:     Vec<_> = seeds.iter().map(|&s| pk_of(s)).collect();
        let msg = b"shared block hash".to_vec();
        let mut sigs: Vec<_> = secrets.iter().map(|s| bls_sign(s, &msg)).collect();
        // Replace signer 1's sig with a sig over a DIFFERENT message — the
        // aggregate must no longer verify under (pk_agg, msg).
        sigs[1] = bls_sign(&secrets[1], b"different message");
        let agg = bls_aggregate(&sigs).unwrap();
        assert!(bls_fast_agg_verify(&pks, &msg, &agg).is_err());
    }

    #[test]
    fn aggregate_rejects_intruder_pubkey() {
        let seeds = [31u8, 32, 33];
        let secrets: Vec<_> = seeds.iter().map(|&s| sk(s)).collect();
        let msg = b"shared block hash".to_vec();
        let sigs: Vec<_> = secrets.iter().map(|s| bls_sign(s, &msg)).collect();
        let agg = bls_aggregate(&sigs).unwrap();
        // Swap signer 1's pubkey for an unrelated one.
        let pks = vec![pk_of(31), pk_of(99), pk_of(33)];
        assert!(bls_fast_agg_verify(&pks, &msg, &agg).is_err());
    }

    #[test]
    fn empty_aggregate_inputs_are_errors() {
        assert!(matches!(bls_aggregate(&[]), Err(BLSError::EmptySignatureSet)));
        assert!(matches!(bls_aggregate_pubkeys(&[]), Err(BLSError::EmptySignatureSet)));
        let sig = bls_sign(&sk(40), b"x");
        assert!(matches!(
            bls_fast_agg_verify(&[], b"x", &BlsAggSignature(*sig.as_bytes())),
            Err(BLSError::EmptySignatureSet)
        ));
    }

    #[test]
    fn forgery_resistance_random_blob_is_rejected() {
        // Pre-Pass-17 the verifier reconstructed an "expected sig" from
        // pubkey[..32] alone and checked the first 32 sig bytes — making
        // this whole scheme trivially forgeable. Post-Pass-17 a random
        // 96-byte blob should fail to even decode as a G2 point, OR fail
        // the pairing check.
        let pk = pk_of(50);
        let bogus = BlsSignature([0xABu8; 96]);
        assert!(!bls_verify_single(&pk, b"some msg", &bogus));
    }

    #[test]
    fn pubkey_from_bytes_rejects_garbage() {
        // 48 bytes of 0xFF is not a valid compressed G1 point.
        assert!(BlsPubKey::from_bytes(&[0xFFu8; 48]).is_err());
        assert!(BlsPubKey::from_bytes(&[0u8; 47]).is_err());
    }

    #[test]
    fn signature_from_bytes_rejects_garbage() {
        // 96 bytes of 0xFF is not a valid compressed G2 point.
        assert!(BlsSignature::from_bytes(&[0xFFu8; 96]).is_err());
        assert!(BlsSignature::from_bytes(&[0u8; 95]).is_err());
    }

    #[test]
    fn proof_of_possession_roundtrip() {
        let secret    = sk(60);
        let pk        = pk_of(60);
        let validator = Address([0xAAu8; 20]);
        let mut msg = Vec::with_capacity(34);
        msg.extend_from_slice(&validator.0);
        msg.extend_from_slice(b"zbx-bls-pop-v1");
        let pop = bls_sign(&secret, &msg);
        assert!(pk.verify_pop(&pop, &validator));
        // Wrong validator → reject.
        assert!(!pk.verify_pop(&pop, &Address([0xBBu8; 20])));
    }

    #[test]
    fn batch_verify_all_valid_and_one_invalid() {
        let m1 = b"m1".to_vec();
        let m2 = b"m2".to_vec();
        let m3 = b"m3".to_vec();
        let triples_ok = vec![
            (pk_of(70), m1.clone(), bls_sign(&sk(70), &m1)),
            (pk_of(71), m2.clone(), bls_sign(&sk(71), &m2)),
            (pk_of(72), m3.clone(), bls_sign(&sk(72), &m3)),
        ];
        assert!(bls_batch_verify(&triples_ok).is_ok());

        let mut bad = triples_ok.clone();
        // Tamper the message of triple 1.
        bad[1].1 = b"tampered".to_vec();
        assert!(matches!(
            bls_batch_verify(&bad),
            Err(BLSError::BatchVerificationFailed(1))
        ));
    }

    #[test]
    fn qc_rejects_out_of_range_bitmap_index() {
        // Pre-Pass-17 architect-review fix regression: a malicious bitmap
        // that claims index 99 (well outside the 4-validator set) used to
        // be silently filtered out, leaving an under-quorum subset to be
        // aggregate-verified — and accepted. Now it must hard-fail with
        // SignerIndexOutOfRange.
        let seeds = [80u8, 81, 82, 83];
        let pks:  Vec<_> = seeds.iter().map(|&s| pk_of(s)).collect();
        let sks_: Vec<_> = seeds.iter().map(|&s| sk(s)).collect();
        let block_hash = H256([0xCDu8; 32]);
        let sigs: Vec<_> = sks_.iter().map(|k| bls_sign(k, &block_hash.0)).collect();
        let agg = bls_aggregate(&sigs).unwrap();
        // Allocate a bitmap big enough to set bit 99, but the validator
        // set only has 4 keys — so 99 is out of range when consulted.
        let mut bm = ValidatorBitmap::new(128);
        bm.set(0); bm.set(1); bm.set(99);
        let validator_keys: Vec<_> = pks.iter()
            .enumerate()
            .map(|(i, pk)| (Address([i as u8; 20]), pk.clone()))
            .collect();
        let qc = BLSQuorumCertificate {
            block_hash,
            round: 7,
            agg_signature: agg,
            signer_bitmap: bm,
        };
        match qc.verify(&validator_keys, 3) {
            Err(BLSError::SignerIndexOutOfRange { idx: 99, n: 4 }) => {}
            other => panic!("expected SignerIndexOutOfRange, got {:?}", other),
        }
    }

    #[test]
    fn quorum_certificate_verifies_with_pairing() {
        let seeds = [80u8, 81, 82, 83];
        let pks:  Vec<_> = seeds.iter().map(|&s| pk_of(s)).collect();
        let sks_: Vec<_> = seeds.iter().map(|&s| sk(s)).collect();
        let block_hash = H256([0xCDu8; 32]);
        let sigs: Vec<_> = sks_.iter().map(|k| bls_sign(k, &block_hash.0)).collect();
        let agg = bls_aggregate(&sigs).unwrap();
        let mut bm = ValidatorBitmap::new(seeds.len());
        for i in 0..seeds.len() { bm.set(i); }
        let validator_keys: Vec<_> = pks.iter()
            .enumerate()
            .map(|(i, pk)| (Address([i as u8; 20]), pk.clone()))
            .collect();
        let qc = BLSQuorumCertificate {
            block_hash,
            round: 7,
            agg_signature: agg,
            signer_bitmap: bm,
        };
        assert!(qc.verify(&validator_keys, 3).is_ok());
    }
}
