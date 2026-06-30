//! BLS12-381 aggregate signatures for the Zebvix validator committee.
//!
//! # Construction (BLS-on-G2, "min-pubkey-size")
//!
//! * Private key  `sk` ∈ Fr (32-byte scalar)
//! * Public key   `pk = sk · g1`        (48-byte compressed G1)
//! * Signature    `σ  = sk · H(msg)`    (96-byte compressed G2)
//!   where `H : {0,1}* → G2` is the IETF RFC 9380 hash-to-curve
//!   `BLS12381G2_XMD:SHA-256_SSWU_RO_` with the ZBX domain-separation tag.
//!
//! Verification: `e(g1, σ) == e(pk, H(msg))`.
//!
//! Same-message aggregation:
//!     `agg_σ  = Σ σ_i  = (Σ sk_i) · H(msg)`
//!     `agg_pk = Σ pk_i = (Σ sk_i) · g1`
//!     ⇒ `e(g1, agg_σ) == e(agg_pk, H(msg))`.
//!
//! This replaces the previous byte-XOR stub, which accepted arbitrary
//! 96-byte blobs as valid "aggregates" and never invoked the pairing.

use crate::keccak::keccak256;
use zbx_types::{error::ZbxError, H256};
use serde_big_array::BigArray;
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop};

use bls12_381::{
    G1Affine, G1Projective, G2Affine, G2Projective, Scalar,
    hash_to_curve::{HashToCurve, ExpandMsgXmd},
};
use group::{Curve, Group};
use ff::Field;
use pairing::PairingCurveAffine;

/// IETF RFC 9380 domain separation tag for hash-to-G2 used by ZBX BLS sigs.
const ZBX_BLS_DST: &[u8] = b"ZBX_BLS_SIG_BLS12381G2_XMD:SHA-256_SSWU_RO_";

/// BLS12-381 private key (Fr scalar, 32 bytes little-endian canonical).
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct BlsPrivKey([u8; 32]);

impl BlsPrivKey {
    pub fn from_bytes(b: &[u8]) -> Result<Self, ZbxError> {
        if b.len() != 32 {
            return Err(ZbxError::InvalidLength { expected: 32, got: b.len() });
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(b);
        // Reject zero — `0 · g1` is the identity, which would trivially
        // verify against a zero signature.
        if arr.iter().all(|&x| x == 0) {
            return Err(ZbxError::Signature("BLS private key must be non-zero".into()));
        }
        // Validate the bytes parse as a canonical Fr element. We use
        // wide reduction so any 32-byte input that's not all zeros is
        // accepted; the field result is uniform over Fr.
        let _scalar = scalar_from_bytes(&arr);
        Ok(BlsPrivKey(arr))
    }

    /// Generate a fresh keypair from an OS-seeded RNG.
    pub fn generate<R: rand::RngCore>(rng: &mut R) -> Self {
        loop {
            let mut bytes = [0u8; 32];
            rng.fill_bytes(&mut bytes);
            if !bytes.iter().all(|&x| x == 0) {
                return BlsPrivKey(bytes);
            }
        }
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    fn scalar(&self) -> Scalar {
        scalar_from_bytes(&self.0)
    }

    /// Derive the 48-byte compressed G1 public key:  `pk = sk · g1`.
    pub fn to_pubkey(&self) -> BlsPubKey {
        let pk_proj = G1Projective::generator() * self.scalar();
        BlsPubKey(pk_proj.to_affine().to_compressed())
    }

    /// Sign a message hash producing a 96-byte compressed G2 signature.
    /// Concretely: `σ = sk · H(msg)` where H uses RFC 9380 XMD:SHA-256.
    pub fn sign(&self, msg: &H256) -> BlsSignature {
        let h_g2 = hash_to_g2(msg.as_ref());
        let sig_proj = h_g2 * self.scalar();
        BlsSignature(sig_proj.to_affine().to_compressed())
    }
}

/// Reduce 32 bytes into an Fr scalar via wide reduction (zero-pad to 64).
fn scalar_from_bytes(b: &[u8; 32]) -> Scalar {
    let mut wide = [0u8; 64];
    wide[..32].copy_from_slice(b);
    Scalar::from_bytes_wide(&wide)
}

/// IETF RFC 9380 hash-to-curve into G2 with the ZBX domain tag.
///
/// Implementation note: `bls12_381 = "0.8"`'s `experimental` feature exposes
/// `HashToCurve` against the `digest 0.9` trait family. Our project-wide
/// `sha2 = "0.10"` Sha256 satisfies `digest 0.10`, not 0.9, so we explicitly
/// reach for `sha2_v09::Sha256` (a thin extra dep, see Cargo.toml comment).
/// The single-message argument is `msg` directly — earlier code wrote `[msg]`
/// which built a `[&[u8]; 1]` and failed `AsRef<[u8]>` selection.
fn hash_to_g2(msg: &[u8]) -> G2Projective {
    <G2Projective as HashToCurve<ExpandMsgXmd<sha2_v09::Sha256>>>::hash_to_curve(
        msg, ZBX_BLS_DST,
    )
}

/// BLS12-381 public key — 48-byte compressed G1 point.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BlsPubKey(#[serde(with = "BigArray")] pub [u8; 48]);

impl BlsPubKey {
    pub fn from_bytes(b: &[u8]) -> Result<Self, ZbxError> {
        if b.len() != 48 {
            return Err(ZbxError::InvalidLength { expected: 48, got: b.len() });
        }
        let mut arr = [0u8; 48];
        arr.copy_from_slice(b);
        // Validate it actually decodes to a curve point.
        let opt = G1Affine::from_compressed(&arr);
        if opt.is_some().into() {
            Ok(BlsPubKey(arr))
        } else {
            Err(ZbxError::Signature("invalid G1 point encoding".into()))
        }
    }

    pub fn as_bytes(&self) -> &[u8; 48] {
        &self.0
    }

    fn to_g1(&self) -> Option<G1Affine> {
        let opt = G1Affine::from_compressed(&self.0);
        if opt.is_some().into() { Some(opt.unwrap()) } else { None }
    }

    /// Fingerprint (keccak256 of the pubkey) for logging.
    pub fn fingerprint(&self) -> String {
        hex::encode(&keccak256(&self.0)[..8])
    }

    /// SEC-2026-05-09 Pass-18 — BLS Proof-of-Possession verification.
    ///
    /// Prevents the rogue-key attack on aggregate BLS signatures:
    /// without a PoP, a malicious validator can publish a "pubkey"
    /// `pk_attacker = pk_real - sum(pk_others)` and then forge an
    /// aggregate-sig that the verifier accepts as signed by the full
    /// committee. Requiring each validator to BLS-sign their *own*
    /// ECDSA address with a fixed domain separator at registration
    /// time forces them to actually possess the matching secret key.
    ///
    /// Domain: `keccak256(validator_address ‖ "zbx-bls-pop-v1")`.
    /// This is the same domain used by `zbx_threshold::BlsPubKey::verify_pop`,
    /// so a PoP produced by either wrapper verifies under both.
    pub fn verify_pop(&self, pop: &BlsSignature, validator: &zbx_types::address::Address) -> bool {
        let mut preimg = Vec::with_capacity(20 + 14);
        preimg.extend_from_slice(validator.as_bytes());
        preimg.extend_from_slice(b"zbx-bls-pop-v1");
        let msg = keccak256(&preimg);
        verify_single(pop, self, &msg)
    }
}

/// BLS12-381 signature — 96-byte compressed G2 point.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlsSignature(#[serde(with = "BigArray")] pub [u8; 96]);

impl BlsSignature {
    pub fn from_bytes(b: &[u8]) -> Result<Self, ZbxError> {
        if b.len() != 96 {
            return Err(ZbxError::InvalidLength { expected: 96, got: b.len() });
        }
        let mut arr = [0u8; 96];
        arr.copy_from_slice(b);
        let opt = G2Affine::from_compressed(&arr);
        if opt.is_some().into() {
            Ok(BlsSignature(arr))
        } else {
            Err(ZbxError::Signature("invalid G2 point encoding".into()))
        }
    }

    pub fn as_bytes(&self) -> &[u8; 96] {
        &self.0
    }

    fn to_g2(&self) -> Option<G2Affine> {
        let opt = G2Affine::from_compressed(&self.0);
        if opt.is_some().into() { Some(opt.unwrap()) } else { None }
    }
}

/// Aggregate multiple BLS signatures into one — real G2 point addition.
/// All signatures must be on the same message for the
/// `verify_aggregate(agg_pk, msg)` form below to apply.
pub fn aggregate_signatures(sigs: &[BlsSignature]) -> Result<BlsSignature, ZbxError> {
    if sigs.is_empty() {
        return Err(ZbxError::Signature("no signatures to aggregate".into()));
    }
    let mut acc = G2Projective::identity();
    for sig in sigs {
        let pt = sig.to_g2().ok_or_else(|| {
            ZbxError::Signature("invalid signature encoding in aggregate input".into())
        })?;
        acc += G2Projective::from(&pt);
    }
    if bool::from(acc.is_identity()) {
        return Err(ZbxError::Signature(
            "aggregated signature is the identity (rogue-key or all-zero input)".into()
        ));
    }
    Ok(BlsSignature(acc.to_affine().to_compressed()))
}

/// Aggregate public keys — real G1 point addition.
pub fn aggregate_pubkeys(pks: &[BlsPubKey]) -> Result<BlsPubKey, ZbxError> {
    if pks.is_empty() {
        return Err(ZbxError::Signature("no pubkeys to aggregate".into()));
    }
    let mut acc = G1Projective::identity();
    for pk in pks {
        let pt = pk.to_g1().ok_or_else(|| {
            ZbxError::Signature("invalid pubkey encoding in aggregate input".into())
        })?;
        acc += G1Projective::from(&pt);
    }
    if bool::from(acc.is_identity()) {
        return Err(ZbxError::Signature(
            "aggregated pubkey is the identity".into()
        ));
    }
    Ok(BlsPubKey(acc.to_affine().to_compressed()))
}

/// Verify an aggregate signature against a list of public keys, all on the
/// same message. Performs the real bilinear pairing check
///
///     e(g1, agg_sig) == e(agg_pk, H(msg))
///
/// where `agg_pk = Σ pk_i`. Returns false on any decoding failure.
pub fn verify_aggregate(
    agg_sig: &BlsSignature,
    pubkeys: &[BlsPubKey],
    msg: &H256,
) -> bool {
    if pubkeys.is_empty() {
        return false;
    }
    let agg_pk = match aggregate_pubkeys(pubkeys) {
        Ok(p)  => p,
        Err(_) => return false,
    };
    verify_single(agg_sig, &agg_pk, msg)
}

/// Verify a single (or pre-aggregated) signature against one public key.
pub fn verify_single(sig: &BlsSignature, pk: &BlsPubKey, msg: &H256) -> bool {
    let sig_g2 = match sig.to_g2() { Some(s) => s, None => return false };
    let pk_g1  = match pk.to_g1()  { Some(p) => p, None => return false };
    if bool::from(sig_g2.is_identity()) || bool::from(pk_g1.is_identity()) {
        return false;
    }
    let h_g2 = hash_to_g2(msg.as_ref()).to_affine();
    let g1   = G1Affine::generator();
    // e(g1, sig) == e(pk, H(msg))
    let lhs = g1.pairing_with(&sig_g2);
    let rhs = pk_g1.pairing_with(&h_g2);
    lhs == rhs
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;
    use zbx_types::H256;

    fn msg_a() -> H256 {
        H256::from(keccak256(b"zbx-bls-test-message-a"))
    }
    fn msg_b() -> H256 {
        H256::from(keccak256(b"zbx-bls-test-message-b"))
    }

    #[test]
    fn keygen_pk_is_valid_g1_point() {
        let sk = BlsPrivKey::generate(&mut OsRng);
        let pk = sk.to_pubkey();
        assert!(BlsPubKey::from_bytes(pk.as_bytes()).is_ok());
    }

    #[test]
    fn sign_and_single_verify_roundtrip() {
        let sk = BlsPrivKey::generate(&mut OsRng);
        let pk = sk.to_pubkey();
        let m = msg_a();
        let sig = sk.sign(&m);
        assert!(verify_single(&sig, &pk, &m));
    }

    #[test]
    fn verify_rejects_wrong_message() {
        let sk = BlsPrivKey::generate(&mut OsRng);
        let pk = sk.to_pubkey();
        let sig = sk.sign(&msg_a());
        assert!(!verify_single(&sig, &pk, &msg_b()));
    }

    #[test]
    fn verify_rejects_wrong_pubkey() {
        let sk1 = BlsPrivKey::generate(&mut OsRng);
        let sk2 = BlsPrivKey::generate(&mut OsRng);
        let m = msg_a();
        let sig = sk1.sign(&m);
        assert!(!verify_single(&sig, &sk2.to_pubkey(), &m));
    }

    #[test]
    fn aggregate_three_signers_same_message() {
        let sks: Vec<_> = (0..3).map(|_| BlsPrivKey::generate(&mut OsRng)).collect();
        let pks: Vec<_> = sks.iter().map(|sk| sk.to_pubkey()).collect();
        let m = msg_a();
        let sigs: Vec<_> = sks.iter().map(|sk| sk.sign(&m)).collect();
        let agg = aggregate_signatures(&sigs).unwrap();
        assert!(verify_aggregate(&agg, &pks, &m));
    }

    #[test]
    fn aggregate_rejects_tampered_signature() {
        let sks: Vec<_> = (0..3).map(|_| BlsPrivKey::generate(&mut OsRng)).collect();
        let pks: Vec<_> = sks.iter().map(|sk| sk.to_pubkey()).collect();
        let m = msg_a();
        let mut sigs: Vec<_> = sks.iter().map(|sk| sk.sign(&m)).collect();
        // Replace one signature with a sig over a DIFFERENT message — aggregate
        // must no longer verify under (agg_pk, msg).
        sigs[1] = sks[1].sign(&msg_b());
        let agg = aggregate_signatures(&sigs).unwrap();
        assert!(!verify_aggregate(&agg, &pks, &m));
    }

    #[test]
    fn aggregate_rejects_wrong_pubkey_set() {
        let sks: Vec<_> = (0..3).map(|_| BlsPrivKey::generate(&mut OsRng)).collect();
        let m = msg_a();
        let sigs: Vec<_> = sks.iter().map(|sk| sk.sign(&m)).collect();
        let agg = aggregate_signatures(&sigs).unwrap();
        // Swap one of the pubkeys for an unrelated one.
        let intruder = BlsPrivKey::generate(&mut OsRng).to_pubkey();
        let pks = vec![sks[0].to_pubkey(), intruder, sks[2].to_pubkey()];
        assert!(!verify_aggregate(&agg, &pks, &m));
    }

    #[test]
    fn empty_aggregate_inputs_are_errors() {
        assert!(aggregate_signatures(&[]).is_err());
        assert!(aggregate_pubkeys(&[]).is_err());
        let sk = BlsPrivKey::generate(&mut OsRng);
        let sig = sk.sign(&msg_a());
        assert!(!verify_aggregate(&sig, &[], &msg_a()));
    }

    #[test]
    fn rejects_zero_private_key() {
        assert!(BlsPrivKey::from_bytes(&[0u8; 32]).is_err());
    }
}
