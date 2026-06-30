//! Verifiable Random Function (VRF) for block proposer selection.
//!
//! Each validator computes VRF(epoch_seed || validator_index) to determine
//! their slot in the round-robin schedule with weighted randomness.

use crate::{keccak::keccak256, secp256k1::PrivKey};
use zbx_types::{error::ZbxError, H256};
use serde_big_array::BigArray;
use serde::{Deserialize, Serialize};

/// VRF proof produced by a validator for a given epoch seed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VrfProof {
    /// The pseudorandom output bytes (32 bytes).
    pub output: H256,
    /// The proof bytes for on-chain verification (64 bytes).
    #[serde(with = "BigArray")]
    pub proof: [u8; 64],
}

/// Compute a VRF output and proof for the given input using the private key.
///
/// ## Construction
///
/// 1. `output = keccak256(privkey_bytes ‖ input)` — pseudorandom, bound to the key.
/// 2. `sign_hash = keccak256("zbx-vrf-v1\x00" ‖ input ‖ output)` — domain-separated.
/// 3. `sig = secp256k1_sign(privkey, sign_hash)` — low-S ECDSA recoverable signature.
/// 4. `proof[0..32] = r`, `proof[32..64] = s` — recovery id omitted (recovered in verify).
pub fn vrf_prove(priv_key: &PrivKey, input: &[u8]) -> VrfProof {
    // Step 1: pseudorandom output bound to the private key.
    let mut key_input = Vec::with_capacity(32 + input.len());
    key_input.extend_from_slice(priv_key.as_bytes());
    key_input.extend_from_slice(input);
    let output = keccak256(&key_input);

    // Step 2: domain-separated hash over (input, output).
    let mut sign_data = Vec::with_capacity(11 + input.len() + 32);
    sign_data.extend_from_slice(b"zbx-vrf-v1\x00");
    sign_data.extend_from_slice(input);
    sign_data.extend_from_slice(output.as_bytes());
    let sign_hash = keccak256(&sign_data);

    // Step 3: secp256k1 ECDSA sign — only the private key holder can do this.
    let sig = priv_key.sign(&sign_hash);
    let sig_bytes = sig.to_bytes();  // r(32) ‖ s(32) ‖ v(1)

    // Step 4: store r‖s; drop v (recovered during verify).
    let mut proof = [0u8; 64];
    proof.copy_from_slice(&sig_bytes[..64]);

    VrfProof { output, proof }
}

/// Verify a VRF proof against the given public key and input.
///
/// ## Construction
///
/// `vrf_prove` computes:
///   - `output = keccak256(privkey_bytes ‖ input)` — the pseudorandom output
///   - `sign_hash = keccak256("zbx-vrf-v1\x00" ‖ input ‖ output)` — domain-separated
///   - `sig = secp256k1_sign(privkey, sign_hash)` — ECDSA recoverable sig
///   - `proof[0..32] = r`, `proof[32..64] = s` (recovery id dropped, recovered here)
///
/// ## Verification
///
/// We recompute `sign_hash` from `(input, proof.output)`, then try both
/// recovery ids (0 and 1).  If either recovers a public key that matches
/// `pub_key_bytes`, the proof is valid and we return `Ok(proof.output)`.
///
/// This is unforgeable: producing a valid (output, proof) for a given
/// `pub_key_bytes` requires knowledge of the corresponding private key.
pub fn vrf_verify(
    pub_key_bytes: &[u8; 65],
    input: &[u8],
    proof: &VrfProof,
) -> Result<H256, ZbxError> {
    use crate::secp256k1::{recover_pubkey, Signature};

    // Recompute the domain-separated hash that was signed during prove.
    let mut sign_data = Vec::with_capacity(11 + input.len() + 32);
    sign_data.extend_from_slice(b"zbx-vrf-v1\x00");
    sign_data.extend_from_slice(input);
    sign_data.extend_from_slice(proof.output.as_bytes());
    let sign_hash = keccak256(&sign_data);

    // Try both recovery ids — the v byte was dropped from the 64-byte proof
    // to keep VrfProof.proof at [u8; 64].  At most one recovery id will yield
    // a valid low-S public key that matches pub_key_bytes.
    for v in [0u8, 1u8] {
        let sig = Signature {
            r: H256::from_slice(&proof.proof[..32]),
            s: H256::from_slice(&proof.proof[32..64]),
            v,
        };
        if let Ok(recovered) = recover_pubkey(&sign_hash, &sig) {
            if recovered.as_bytes() == pub_key_bytes {
                return Ok(proof.output);
            }
        }
    }

    Err(ZbxError::Signature(
        "VRF proof verification failed: recovered public key does not match".into(),
    ))
}

/// Compute a deterministic 256-bit "stake-weighted ticket" for ranking.
///
/// Audit-2026-05-01 S7-CR5: previously `vrf_score` used `f64.powf(...)`
/// for ranking, which is non-deterministic across architectures /
/// libm versions and silently forks the chain when validators on
/// different CPUs compute different scores. Replaced with deterministic
/// integer arithmetic: ticket = vrf_output_u256 / stake_weight. Higher
/// stake → smaller ticket → higher selection priority (lowest ticket
/// wins). All operations are exact U256, identical on every platform.
///
/// Returns 32 raw bytes (big-endian U256) so callers can rank with
/// `ticket_bytes_a.cmp(&ticket_bytes_b)` and get total ordering.
pub fn vrf_ticket(output: &H256, stake_weight: u64) -> [u8; 32] {
    use zbx_types::U256;
    let raw = U256::from_big_endian(output.as_bytes());
    let weight = stake_weight.max(1); // guard against zero-stake div-by-zero
    let ticket = raw / U256::from(weight);
    let mut out = [0u8; 32];
    ticket.to_big_endian(&mut out);
    out
}

/// Select the block proposer from a validator set deterministically.
/// The validator with the **lowest** stake-weighted ticket wins.
///
/// Audit-2026-05-01 S7-CR5: rewritten with integer ticket comparison;
/// see `vrf_ticket` for the rationale.
pub fn select_proposer(vrf_outputs: &[H256], stake_weights: &[u64]) -> usize {
    assert_eq!(vrf_outputs.len(), stake_weights.len());
    assert!(!vrf_outputs.is_empty(), "select_proposer requires ≥1 validator");
    let mut best_idx = 0usize;
    let mut best_ticket = vrf_ticket(&vrf_outputs[0], stake_weights[0]);
    for (i, (output, &weight)) in vrf_outputs.iter().zip(stake_weights).enumerate().skip(1) {
        let t = vrf_ticket(output, weight);
        if t < best_ticket {
            best_ticket = t;
            best_idx = i;
        }
    }
    best_idx
}

#[cfg(test)]
mod consensus_vrf_tests {
    use super::*;
    use crate::secp256k1::PrivKey;

    #[test]
    fn prove_then_verify_succeeds() {
        let key = PrivKey::random();
        let pubkey = key.to_pubkey();
        let input = b"epoch:42:validator:7";

        let proof = vrf_prove(&key, input);
        let result = vrf_verify(pubkey.as_bytes(), input, &proof);
        assert!(result.is_ok(), "valid proof must verify: {:?}", result);
        assert_eq!(result.unwrap(), proof.output);
    }

    #[test]
    fn verify_wrong_pubkey_fails() {
        let key = PrivKey::random();
        let wrong_key = PrivKey::random();
        let input = b"epoch:42:validator:7";

        let proof = vrf_prove(&key, input);
        let result = vrf_verify(wrong_key.to_pubkey().as_bytes(), input, &proof);
        assert!(result.is_err(), "wrong pubkey must be rejected");
    }

    #[test]
    fn verify_wrong_input_fails() {
        let key = PrivKey::random();
        let input = b"epoch:42:validator:7";
        let wrong_input = b"epoch:42:validator:8";

        let proof = vrf_prove(&key, input);
        let result = vrf_verify(key.to_pubkey().as_bytes(), wrong_input, &proof);
        assert!(result.is_err(), "wrong input must be rejected");
    }

    #[test]
    fn tampered_output_fails() {
        let key = PrivKey::random();
        let input = b"epoch:42:validator:7";

        let mut proof = vrf_prove(&key, input);
        // Flip a bit in the output
        proof.output.0[0] ^= 0x01;
        let result = vrf_verify(key.to_pubkey().as_bytes(), input, &proof);
        assert!(result.is_err(), "tampered output must be rejected");
    }

    #[test]
    fn tampered_proof_bytes_fails() {
        let key = PrivKey::random();
        let input = b"epoch:42:validator:7";

        let mut proof = vrf_prove(&key, input);
        // Corrupt the r component of the signature
        proof.proof[0] ^= 0xff;
        let result = vrf_verify(key.to_pubkey().as_bytes(), input, &proof);
        assert!(result.is_err(), "tampered proof bytes must be rejected");
    }

    #[test]
    fn output_is_deterministic() {
        let key = PrivKey::random();
        let input = b"epoch:42:validator:7";

        let p1 = vrf_prove(&key, input);
        let p2 = vrf_prove(&key, input);
        assert_eq!(p1.output, p2.output, "VRF output must be deterministic");
        assert_eq!(p1.proof, p2.proof, "VRF proof must be deterministic");
    }

    #[test]
    fn different_inputs_give_different_outputs() {
        let key = PrivKey::random();
        let p1 = vrf_prove(&key, b"input-one");
        let p2 = vrf_prove(&key, b"input-two");
        assert_ne!(p1.output, p2.output, "different inputs must yield different outputs");
    }
}

/// ECVRF-EDWARDS25519-SHA512 (RFC 9381) — precompile 0x0E surface.
///
/// ## Implementation
///
/// Implements the RFC 9381 §5.3 ECVRF_verify equation:
///
/// ```text
/// Gamma || c || s  =  ECVRF_decode_proof(pi_string)
/// H                =  ECVRF_hash_to_try_and_increment(suite, Y, alpha)
/// U                =  s·B − c·Y
/// V                =  s·H − c·Γ
/// c'               =  ECVRF_hash_points(H, Γ, U, V)[0..16]
/// if c == c' → beta = SHA-512(suite ‖ 0x03 ‖ compress(Γ) ‖ 0x00)
/// ```
///
/// The hash-to-curve step uses SHA-512 try-and-increment (TAI, RFC 9381 §5.4.1.2)
/// which is internally consistent — provers and verifiers in this codebase both
/// use the same method. Replacing the inner loop with RFC 9380 Elligator 2 is
/// the only delta needed for full RFC 9381 suite 0x04 (ELL2) compliance.
pub mod ecvrf_edwards25519 {
    use curve25519_dalek::edwards::{CompressedEdwardsY, EdwardsPoint};
    use curve25519_dalek::scalar::Scalar;
    use curve25519_dalek::traits::IsIdentity;
    use sha2::{Digest, Sha512};

    /// Suite string byte (used as a domain-separation prefix in every hash call).
    pub const SUITE_STRING: u8 = 0x04;
    /// Length of `pi_string` (Gamma:32 || c:16 || s:32).
    pub const PROOF_LEN: usize = 80;
    /// Length of `beta_string` (SHA-512 output = 64 bytes).
    pub const BETA_LEN: usize = 64;
    /// Length of the compressed Edwards25519 public-key encoding.
    pub const PUBKEY_LEN: usize = 32;

    // ── RFC 9381 §5.4.1.2 — ECVRF_hash_to_try_and_increment ──────────────────

    /// Map `(Y, alpha)` to a curve point using SHA-512 try-and-increment.
    ///
    /// Iterates counter bytes 0x00..0xFF, hashing suite ‖ 0x01 ‖ Y ‖ alpha ‖ ctr
    /// with SHA-512 each round. The first 32 bytes of the digest are tried as a
    /// compressed Edwards25519 point; if they decompress and the cofactor-cleared
    /// result is not the identity, that point is returned.
    fn hash_to_try_and_increment(pk: &[u8; PUBKEY_LEN], alpha: &[u8]) -> Option<EdwardsPoint> {
        for ctr in 0u8..=255 {
            let hash = Sha512::new()
                .chain_update([SUITE_STRING, 0x01])
                .chain_update(pk)
                .chain_update(alpha)
                .chain_update([ctr])
                .finalize();

            // Attempt to interpret the first 32 bytes as a compressed Edwards point.
            let mut candidate = [0u8; 32];
            candidate.copy_from_slice(&hash[0..32]);

            if let Some(point) = CompressedEdwardsY(candidate).decompress() {
                let cleared = point.mul_by_cofactor();
                if !cleared.is_identity() {
                    return Some(cleared);
                }
            }
        }
        None  // astronomically unlikely: all 256 hash outputs are off-curve
    }

    // ── RFC 9381 §5.4.3 — ECVRF_hash_points ─────────────────────────────────

    /// Compute the challenge scalar c' from four curve points.
    ///
    /// `c' = SHA-512(suite ‖ 0x02 ‖ H ‖ Γ ‖ U ‖ V ‖ 0x00)[0..16]`
    fn hash_points(
        h: &EdwardsPoint,
        gamma: &EdwardsPoint,
        u: &EdwardsPoint,
        v: &EdwardsPoint,
    ) -> [u8; 16] {
        let digest = Sha512::new()
            .chain_update([SUITE_STRING, 0x02])
            .chain_update(h.compress().as_bytes())
            .chain_update(gamma.compress().as_bytes())
            .chain_update(u.compress().as_bytes())
            .chain_update(v.compress().as_bytes())
            .chain_update([0x00])
            .finalize();
        let mut c = [0u8; 16];
        c.copy_from_slice(&digest[0..16]);
        c
    }

    // ── RFC 9381 §5.2 — ECVRF_proof_to_hash ─────────────────────────────────

    /// Derive `beta_string` from a verified Γ point.
    ///
    /// `beta = SHA-512(suite ‖ 0x03 ‖ compress(Γ) ‖ 0x00)`
    fn proof_to_hash(gamma: &EdwardsPoint) -> [u8; BETA_LEN] {
        let digest = Sha512::new()
            .chain_update([SUITE_STRING, 0x03])
            .chain_update(gamma.compress().as_bytes())
            .chain_update([0x00])
            .finalize();
        let mut beta = [0u8; BETA_LEN];
        beta.copy_from_slice(&digest);
        beta
    }

    // ── RFC 9381 §5.3 — ECVRF_verify ─────────────────────────────────────────

    /// RFC 9381 §5.3 `ECVRF_verify(Y, alpha_string, pi_string)`.
    ///
    /// Returns `Some(beta)` iff `pi_string` is a valid VRF proof for
    /// `(Y, alpha_string)`.  Returns `None` for any decode failure or
    /// verification mismatch — the dispatcher converts `None` to a
    /// 32-byte zero output, matching the ECRECOVER convention.
    ///
    /// ## Verification steps
    ///
    /// 1. `pi_string` must be exactly `PROOF_LEN` (80) bytes.
    /// 2. Decode: Γ (bytes 0..32), c (bytes 32..48), s (bytes 48..80).
    /// 3. Γ must decompress to a valid Edwards25519 point.
    /// 4. s must be a canonical scalar (< L, the group order).
    /// 5. Y must decompress; cofactor-cleared Y must not be the identity.
    /// 6. H = hash_to_try_and_increment(Y, alpha).
    /// 7. U = s·B − c·Y  (B = basepoint).
    /// 8. V = s·H − c·Γ.
    /// 9. c′ = hash_points(H, Γ, U, V)[0..16].
    /// 10. Accept iff c == c′; then beta = proof_to_hash(Γ).
    pub fn verify(pubkey: &[u8; PUBKEY_LEN], alpha: &[u8], pi: &[u8]) -> Option<[u8; BETA_LEN]> {
        // Step 1 — length check.
        if pi.len() != PROOF_LEN {
            return None;
        }

        // Step 2 — decode proof components.
        let mut gamma_bytes = [0u8; 32];
        gamma_bytes.copy_from_slice(&pi[0..32]);
        let mut c_bytes = [0u8; 16];
        c_bytes.copy_from_slice(&pi[32..48]);
        let mut s_bytes = [0u8; 32];
        s_bytes.copy_from_slice(&pi[48..80]);

        // Step 3 — Γ decompression.
        let gamma = CompressedEdwardsY(gamma_bytes).decompress()?;

        // Step 4 — s must be a canonical scalar (< L).
        let s_scalar = Option::<Scalar>::from(Scalar::from_canonical_bytes(s_bytes))?;

        // c is 16 bytes; pad to 32 bytes LE before constructing a scalar.
        // c < 2^128 < L (group order ≈ 2^252), so this is always canonical.
        let mut c_scalar_bytes = [0u8; 32];
        c_scalar_bytes[..16].copy_from_slice(&c_bytes);
        let c_scalar = Option::<Scalar>::from(Scalar::from_canonical_bytes(c_scalar_bytes))?;

        // Step 5 — Y decompression and small-subgroup check.
        let y_point = CompressedEdwardsY(*pubkey).decompress()?;
        if y_point.mul_by_cofactor().is_identity() {
            return None;
        }

        // Step 6 — H = hash_to_curve(Y, alpha).
        let h_point = hash_to_try_and_increment(pubkey, alpha)?;

        // Steps 7 & 8 — compute U and V.
        // U = s·B − c·Y  =  s·B + (−c)·Y
        // V = s·H − c·Γ  =  s·H + (−c)·Γ
        use curve25519_dalek::constants::ED25519_BASEPOINT_POINT;
        let neg_c = -c_scalar;
        let u_point: EdwardsPoint = s_scalar * ED25519_BASEPOINT_POINT + neg_c * y_point;
        let v_point: EdwardsPoint = s_scalar * h_point + neg_c * gamma;

        // Step 9 — recompute challenge.
        let c_prime = hash_points(&h_point, &gamma, &u_point, &v_point);

        // Step 10 — verify challenge matches; derive beta if so.
        if c_bytes != c_prime {
            return None;
        }
        Some(proof_to_hash(&gamma))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use sha2::{Digest as Sha2Digest, Sha512 as Sha512Test};

        // A well-formed (Γ, c, s) produced by our own hash_to_try_and_increment prover.
        // Used as the positive-path baseline for all mutation tests.
        fn self_consistent_proof() -> ([u8; 32], Vec<u8>) {
            // Private scalar a = 1 (smallest non-trivial key).
            // Public key Y = 1·B.
            use curve25519_dalek::constants::ED25519_BASEPOINT_POINT;
            use curve25519_dalek::scalar::Scalar;

            let a_bytes: [u8; 32] = {
                let mut b = [0u8; 32];
                b[0] = 1;
                b
            };
            let a_scalar = Option::<Scalar>::from(Scalar::from_canonical_bytes(a_bytes)).unwrap();
            let y_point  = a_scalar * ED25519_BASEPOINT_POINT;
            let pk: [u8; 32] = y_point.compress().to_bytes();

            let alpha = b"zbx-vrf-test";

            // H = hash_to_try_and_increment(pk, alpha)
            let h_point = hash_to_try_and_increment(&pk, alpha).unwrap();

            // Γ = a·H
            let gamma_point = a_scalar * h_point;
            let gamma_bytes = gamma_point.compress().to_bytes();

            // k (nonce) = deterministic scalar derived from priv + alpha.
            // Clear the top 4 bits of byte 31 so k < 2^252 < L (group order),
            // guaranteeing Scalar::from_canonical_bytes succeeds without mod reduction.
            let k_seed = Sha512Test::new()
                .chain_update(a_bytes)
                .chain_update(alpha)
                .finalize();
            let mut k_bytes = [0u8; 32];
            k_bytes.copy_from_slice(&k_seed[0..32]);
            k_bytes[31] &= 0x0f;  // L ≈ 2^252, so clearing top 4 bits → k < 2^252 < L
            let k_scalar = Option::<Scalar>::from(Scalar::from_canonical_bytes(k_bytes))
                .expect("forced-canonical k_bytes must yield a valid scalar");

            // U_commit = k·B,  V_commit = k·H
            let u_commit = k_scalar * ED25519_BASEPOINT_POINT;
            let v_commit = k_scalar * h_point;

            // c = hash_points(H, Γ, U_commit, V_commit)[0..16]
            let c_bytes = hash_points(&h_point, &gamma_point, &u_commit, &v_commit);

            // s = k + c·a  (mod L)   — RFC 9381 §5.1 sign convention.
            // The verifier uses U = s·B − c·Y which expands to k·B only with +:
            //   U = (k + c·a)·B − c·(a·B) = k·B + c·a·B − c·a·B = k·B ✓
            let mut c_full = [0u8; 32];
            c_full[..16].copy_from_slice(&c_bytes);
            let c_scalar = Option::<Scalar>::from(Scalar::from_canonical_bytes(c_full)).unwrap();
            let s_scalar = k_scalar + c_scalar * a_scalar;

            // Pack proof: Γ(32) || c(16) || s(32)
            let mut pi = Vec::with_capacity(PROOF_LEN);
            pi.extend_from_slice(&gamma_bytes);
            pi.extend_from_slice(&c_bytes);
            pi.extend_from_slice(s_scalar.as_bytes());

            (pk, pi)
        }

        #[test]
        fn self_consistent_proof_verifies() {
            let (pk, pi) = self_consistent_proof();
            assert!(verify(&pk, b"zbx-vrf-test", &pi).is_some(),
                "self-consistent proof must verify");
        }

        #[test]
        fn wrong_alpha_rejected() {
            let (pk, pi) = self_consistent_proof();
            assert!(verify(&pk, b"different-alpha", &pi).is_none(),
                "proof for different alpha must be rejected");
        }

        #[test]
        fn malformed_proof_length_rejected() {
            let (pk, _) = self_consistent_proof();
            assert!(verify(&pk, b"zbx-vrf-test", &vec![0u8; 79]).is_none());
            assert!(verify(&pk, b"zbx-vrf-test", &vec![0u8; 81]).is_none());
        }

        #[test]
        fn non_canonical_scalar_rejected() {
            let (pk, mut pi) = self_consistent_proof();
            // s = all-0xff (>> L) — must fail canonical check.
            for b in pi[48..80].iter_mut() { *b = 0xff; }
            assert!(verify(&pk, b"zbx-vrf-test", &pi).is_none());
        }

        #[test]
        fn small_subgroup_pubkey_rejected() {
            let (_, pi) = self_consistent_proof();
            // The Edwards25519 identity compresses to [1, 0, 0, …, 0].
            let mut pk = [0u8; 32];
            pk[0] = 0x01;
            assert!(verify(&pk, b"zbx-vrf-test", &pi).is_none());
        }

        #[test]
        fn flipped_gamma_byte_rejected() {
            let (pk, mut pi) = self_consistent_proof();
            pi[0] ^= 0x01;  // corrupt Γ
            assert!(verify(&pk, b"zbx-vrf-test", &pi).is_none());
        }

        #[test]
        fn beta_is_deterministic() {
            let (pk, pi) = self_consistent_proof();
            let b1 = verify(&pk, b"zbx-vrf-test", &pi);
            let b2 = verify(&pk, b"zbx-vrf-test", &pi);
            assert_eq!(b1, b2, "beta must be deterministic across two calls");
        }
    }
}
