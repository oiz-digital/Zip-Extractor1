//! Signature verification for FROST threshold Schnorr signatures.
//!
//! ## FROST Schnorr Verification Equation
//!
//! Given aggregated signature (R, s), message m, and group public key PK:
//!
//!   e = SHA-256(R_compressed || PK_compressed || m)   — Fiat-Shamir challenge
//!   Valid iff:  s·G == R + e·PK                       — Schnorr equation
//!
//! This is identical to a standard Schnorr signature — the threshold origin
//! is transparent to the verifier (FROST design goal).

use crate::aggregate::ThresholdSig;
use crate::keyshare::GroupKey;

/// Verify an aggregated threshold Schnorr signature against a message and group key.
///
/// Implements the FROST Schnorr equation: `s·G == R + e·PK`
/// where `e = SHA-256(R_compressed || PK_compressed || message)`.
///
/// Returns `false` for:
///   - All-zero R or s (structurally invalid / point at infinity)
///   - R or PK bytes that are not valid secp256k1 curve points
///   - s that is not a valid secp256k1 field scalar
///   - Schnorr equation check failure
pub fn verify_threshold_sig(
    sig: &ThresholdSig,
    message: &[u8],
    group_key: &GroupKey,
) -> bool {
    use k256::{
        elliptic_curve::{
            sec1::FromEncodedPoint,
            PrimeField,
        },
        AffinePoint, EncodedPoint, FieldBytes, ProjectivePoint, Scalar,
    };
    use sha2::{Digest, Sha256};

    // ── Structural guards ────────────────────────────────────────────────────
    if sig.R.iter().all(|&b| b == 0) || sig.s.iter().all(|&b| b == 0) {
        return false; // point at infinity or zero scalar — invalid
    }

    // ── Parse R (aggregate nonce commitment) — 33-byte compressed SEC1 ──────
    let r_encoded = match EncodedPoint::from_bytes(&sig.R) {
        Ok(ep) => ep,
        Err(_) => return false,
    };
    let r_affine_ct = AffinePoint::from_encoded_point(&r_encoded);
    if r_affine_ct.is_none().into() {
        return false; // not a valid curve point
    }
    let R = ProjectivePoint::from(r_affine_ct.unwrap());

    // ── Parse group public key (PK) — 33-byte compressed SEC1 ───────────────
    let pk_bytes = group_key.to_bytes();
    let pk_encoded = match EncodedPoint::from_bytes(&pk_bytes) {
        Ok(ep) => ep,
        Err(_) => return false,
    };
    let pk_affine_ct = AffinePoint::from_encoded_point(&pk_encoded);
    if pk_affine_ct.is_none().into() {
        return false; // not a valid curve point
    }
    let PK = ProjectivePoint::from(pk_affine_ct.unwrap());

    // ── Parse s as a secp256k1 scalar ────────────────────────────────────────
    let s_repr: FieldBytes = sig.s.into();
    let s_ct = Scalar::from_repr(s_repr);
    if s_ct.is_none().into() {
        return false; // value >= n (invalid scalar)
    }
    let s: Scalar = s_ct.unwrap();

    // ── Compute Fiat-Shamir challenge e = SHA-256(R || PK || m) ─────────────
    let mut h = Sha256::new();
    h.update(&sig.R);    // 33 bytes: R compressed
    h.update(&pk_bytes); // 33 bytes: PK compressed
    h.update(message);
    let e_hash = h.finalize();

    // Reduce e hash to a valid scalar (SHA-256 output may exceed secp256k1 n).
    // Try direct conversion first; fall back to zeroing the high byte if needed
    // (hash >> 8 is always < n since n ≈ 2²⁵⁶ - 4.3·10³⁸).
    let e_repr: FieldBytes = e_hash.into();
    let e_ct = Scalar::from_repr(e_repr);
    let e: Scalar = if e_ct.is_some().into() {
        e_ct.unwrap()
    } else {
        let mut reduced = e_hash;
        reduced[0] = 0; // shift right by 1 byte
        let repr2: FieldBytes = reduced.into();
        match Scalar::from_repr(repr2) {
            ct if ct.is_some().into() => ct.unwrap(),
            _ => return false,
        }
    };

    // ── Schnorr check: s·G == R + e·PK ──────────────────────────────────────
    let lhs = ProjectivePoint::GENERATOR * s; // s·G
    let rhs = R + PK * e;                     // R + e·PK
    lhs == rhs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate::ThresholdSig;
    use crate::keyshare::GroupKey;
    use crate::scalar::{bytes_to_scalar_reduce, point_to_compressed, scalar_to_bytes};
    use k256::{ProjectivePoint, Scalar};
    use sha2::{Sha256, Digest};

    /// Produce a valid FROST Schnorr (R, s) pair for `message` using secret
    /// scalar `sk` and nonce `k_nonce`.  Both must be non-zero.
    fn make_valid_sig(sk: Scalar, k_nonce: Scalar, message: &[u8]) -> (ThresholdSig, GroupKey) {
        let pk_point = ProjectivePoint::GENERATOR * sk;
        let r_point  = ProjectivePoint::GENERATOR * k_nonce;

        let pk_compressed = point_to_compressed(&pk_point);
        let r_compressed  = point_to_compressed(&r_point);

        // e = SHA-256(R || PK || message) reduced mod n — mirrors verify.rs
        let mut h = Sha256::new();
        h.update(&r_compressed);
        h.update(&pk_compressed);
        h.update(message);
        let e_hash: [u8; 32] = h.finalize().into();
        let e = bytes_to_scalar_reduce(&e_hash);

        // s = k + e·sk  (mod n)
        let s = k_nonce + e * sk;

        let sig = ThresholdSig {
            R:       r_compressed,
            s:       scalar_to_bytes(&s),
            signers: vec![1],
        };
        let gk = GroupKey::from_bytes(pk_compressed);
        (sig, gk)
    }

    // ── happy-path / round-trip tests ────────────────────────────────────────

    #[test]
    fn valid_schnorr_sig_passes() {
        let message = b"zbx-chain threshold test 2026";
        let (sig, gk) = make_valid_sig(Scalar::from(7u64), Scalar::from(13u64), message);
        assert!(verify_threshold_sig(&sig, message, &gk));
    }

    #[test]
    fn multiple_distinct_key_nonce_pairs_all_pass() {
        let cases: &[(&[u8], u64, u64)] = &[
            (b"alpha", 3 * 17, 5 * 31),
            (b"beta",  11,     101),
            (b"gamma", 9999,   31337),
            (b"delta", 2,      65537),
        ];
        for (msg, sk_val, k_val) in cases {
            let (sig, gk) = make_valid_sig(Scalar::from(*sk_val), Scalar::from(*k_val), msg);
            assert!(
                verify_threshold_sig(&sig, msg, &gk),
                "failed for msg={:?} sk={sk_val} k={k_val}", msg
            );
        }
    }

    #[test]
    fn verify_is_deterministic() {
        let message = b"determinism check";
        let (sig, gk) = make_valid_sig(Scalar::from(42u64), Scalar::from(99u64), message);
        assert!(verify_threshold_sig(&sig, message, &gk));
        assert!(verify_threshold_sig(&sig, message, &gk),
            "second call must return the same result");
    }

    // ── tamper tests — every mutation must cause failure ─────────────────────

    #[test]
    fn tampered_message_fails() {
        let message = b"original message";
        let (sig, gk) = make_valid_sig(Scalar::from(42u64), Scalar::from(99u64), message);
        assert!(!verify_threshold_sig(&sig, b"tampered message", &gk));
    }

    #[test]
    fn tampered_r_fails() {
        let message = b"test payload";
        let (mut sig, gk) = make_valid_sig(Scalar::from(11u64), Scalar::from(17u64), message);
        sig.R[1] ^= 0xff;
        assert!(!verify_threshold_sig(&sig, message, &gk));
    }

    #[test]
    fn tampered_s_fails() {
        let message = b"test payload";
        let (mut sig, gk) = make_valid_sig(Scalar::from(11u64), Scalar::from(17u64), message);
        sig.s[15] ^= 0x01;
        assert!(!verify_threshold_sig(&sig, message, &gk));
    }

    #[test]
    fn wrong_group_key_fails() {
        let message = b"test payload";
        let sk1 = Scalar::from(11u64);
        let sk2 = Scalar::from(99u64);
        let (sig, _gk1) = make_valid_sig(sk1, Scalar::from(17u64), message);
        // Build a group key from a different secret — verification must fail
        let wrong_pk = point_to_compressed(&(ProjectivePoint::GENERATOR * sk2));
        let gk_wrong = GroupKey::from_bytes(wrong_pk);
        assert!(!verify_threshold_sig(&sig, message, &gk_wrong));
    }

    // ── edge / degenerate inputs ─────────────────────────────────────────────

    #[test]
    fn all_zero_r_is_rejected() {
        let sk = Scalar::from(7u64);
        let pk = point_to_compressed(&(ProjectivePoint::GENERATOR * sk));
        let gk = GroupKey::from_bytes(pk);
        let sig = ThresholdSig { R: [0u8; 33], s: [1u8; 32], signers: vec![1] };
        // [0u8; 33] is the identity-point encoding — must be rejected
        assert!(!verify_threshold_sig(&sig, b"msg", &gk));
    }

    #[test]
    fn all_zero_s_is_rejected() {
        let message = b"test";
        let (mut sig, gk) = make_valid_sig(Scalar::from(7u64), Scalar::from(13u64), message);
        sig.s = [0u8; 32]; // s = 0 means s·G = identity, which cannot equal R + e·PK
        assert!(!verify_threshold_sig(&sig, message, &gk));
    }

    #[test]
    fn empty_message_is_handled() {
        // An empty message is unusual but must not panic
        let (sig, gk) = make_valid_sig(Scalar::from(7u64), Scalar::from(13u64), b"");
        assert!(verify_threshold_sig(&sig, b"", &gk));
    }

    #[test]
    fn sig_valid_for_one_message_fails_for_another() {
        let msg_a = b"message A";
        let msg_b = b"message B";
        let (sig, gk) = make_valid_sig(Scalar::from(7u64), Scalar::from(13u64), msg_a);
        assert!(verify_threshold_sig(&sig, msg_a, &gk),  "sig must pass for msg_a");
        assert!(!verify_threshold_sig(&sig, msg_b, &gk), "sig must fail for msg_b");
    }
}
