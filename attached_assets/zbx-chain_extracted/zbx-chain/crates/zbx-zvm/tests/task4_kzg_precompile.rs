//! Task #4 (Precompile 0x0B — EIP-4844 KZG point evaluation): ZVM-side tests.
//!
//! These tests exercise the ZVM precompile path through
//! `do_kzg_with_settings` (bypasses the global `OnceLock` so each test
//! supplies its own trusted setup). The cross-VM consensus-equivalence
//! test in `zbx-evm`'s suite asserts the two engines produce the same
//! bytes for the same inputs/setup.

use bls12_381::{G1Affine, G1Projective, G2Projective, Scalar};
use ff::Field;
use group::Curve;
use rand::rngs::OsRng;

use zbx_crypto::kzg::{
    do_kzg_point_eval, kzg_to_versioned_hash, point_evaluation_success_return,
    KzgError, KzgSettings, BLS_MODULUS_BE,
};
use zbx_zvm::precompiles::do_kzg_with_settings;

// --- Test helpers (degree-1 polynomial p(X) = a + b·X with known secret s).

fn test_setup(s: Scalar) -> (KzgSettings, G1Affine) {
    let s_g2 = (G2Projective::generator() * s).to_affine();
    let s_g1 = (G1Projective::generator() * s).to_affine();
    (KzgSettings { s_g2 }, s_g1)
}

fn commit_deg1(a: Scalar, b: Scalar, s_g1: &G1Affine) -> G1Affine {
    (G1Projective::generator() * a + G1Projective::from(*s_g1) * b).to_affine()
}

fn proof_deg1(b: Scalar) -> G1Affine {
    (G1Projective::generator() * b).to_affine()
}

fn scalar_to_be32(s: &Scalar) -> [u8; 32] {
    let le = s.to_bytes();
    let mut be = [0u8; 32];
    for (i, x) in le.iter().enumerate() {
        be[31 - i] = *x;
    }
    be
}

/// Build a complete 192-byte EIP-4844 input (versioned-hash bound).
fn build_input(c: &G1Affine, z: &Scalar, y: &Scalar, pi: &G1Affine) -> Vec<u8> {
    let c_b = c.to_compressed();
    let pi_b = pi.to_compressed();
    let vh = kzg_to_versioned_hash(&c_b);
    let mut input = Vec::with_capacity(192);
    input.extend_from_slice(&vh);
    input.extend_from_slice(&scalar_to_be32(z));
    input.extend_from_slice(&scalar_to_be32(y));
    input.extend_from_slice(&c_b);
    input.extend_from_slice(&pi_b);
    input
}

fn make_valid(s: Scalar) -> (KzgSettings, Vec<u8>) {
    let (settings, s_g1) = test_setup(s);
    let a = Scalar::from(7u64);
    let b = Scalar::from(3u64);
    let z = Scalar::from(11u64);
    let y = a + b * z;
    let c = commit_deg1(a, b, &s_g1);
    let pi = proof_deg1(b);
    (settings, build_input(&c, &z, &y, &pi))
}

#[test]
fn kzg_zvm_valid_proof_returns_success_constants() {
    let s = Scalar::random(&mut OsRng);
    let (settings, input) = make_valid(s);

    let (out, gas_used) = do_kzg_with_settings(&input, 100_000, &settings)
        .expect("valid proof must succeed");
    assert_eq!(gas_used, 50_000);
    assert_eq!(out, point_evaluation_success_return());
    // FIELD_ELEMENTS_PER_BLOB = 4096 = 0x1000; right-aligned in word[0..32].
    assert_eq!(out[30], 0x10);
    assert_eq!(out[31], 0x00);
    assert_eq!(&out[32..64], &BLS_MODULUS_BE);
}

#[test]
fn kzg_zvm_wrong_versioned_hash_reverts() {
    let s = Scalar::random(&mut OsRng);
    let (settings, mut input) = make_valid(s);
    // Flip a byte in the versioned-hash region (still keep first byte = 0x01
    // so the failure is on hash mismatch, not the domain prefix check —
    // both surfaces map to KzgError::VersionedHash).
    input[5] ^= 0xFF;
    let res = do_kzg_with_settings(&input, 100_000, &settings);
    assert!(res.is_err(), "tampered versioned hash must revert: {:?}", res);
    let msg = format!("{:?}", res.unwrap_err());
    assert!(msg.contains("versioned hash"), "got: {msg}");
}

#[test]
fn kzg_zvm_tampered_proof_reverts() {
    let s = Scalar::random(&mut OsRng);
    let (settings, mut input) = make_valid(s);
    // Flip a low bit in the proof region. Compressed G1 has the high bit
    // set as a flag, so flipping a low bit usually still decodes to a
    // valid (but wrong) G1 point. If by accident we hit an invalid encoding
    // the precompile reverts via BadProof, which is also a hard fail.
    let proof_off = 192 - 48;
    input[proof_off + 47] ^= 0x01;
    let res = do_kzg_with_settings(&input, 100_000, &settings);
    assert!(res.is_err(), "tampered proof must revert: {:?}", res);
}

#[test]
fn kzg_zvm_wrong_commitment_reverts_via_versioned_hash_binding() {
    let s = Scalar::random(&mut OsRng);
    let (settings, mut input) = make_valid(s);
    // Flip a byte inside the commitment region. The versioned-hash
    // binding catches this BEFORE the pairing check runs (the input
    // versioned hash no longer matches sha256(commitment)).
    let c_off = 96;
    input[c_off + 30] ^= 0x01;
    let res = do_kzg_with_settings(&input, 100_000, &settings);
    assert!(res.is_err(), "tampered commitment must revert: {:?}", res);
}

#[test]
fn kzg_zvm_wrong_length_reverts() {
    let s = Scalar::random(&mut OsRng);
    let (settings, input) = make_valid(s);
    // Truncated.
    let res = do_kzg_with_settings(&input[..191], 100_000, &settings);
    assert!(res.is_err());
    // Overlong.
    let mut long = input.clone();
    long.push(0u8);
    let res2 = do_kzg_with_settings(&long, 100_000, &settings);
    assert!(res2.is_err());
}

#[test]
fn kzg_zvm_out_of_gas_returns_oog_not_revert() {
    let s = Scalar::random(&mut OsRng);
    let (settings, input) = make_valid(s);
    let res = do_kzg_with_settings(&input, 49_999, &settings);
    assert!(matches!(res, Err(zbx_zvm::error::ZvmError::OutOfGas)),
        "expected OOG, got {:?}", res);
}

#[test]
fn kzg_zvm_no_global_setup_fails_closed() {
    // Direct dispatcher call goes through the OnceLock path. We can't
    // safely reset the global between tests in the same binary (the
    // crypto-side test binary may have set it), so verify by exercising
    // the address dispatcher with a fresh process-state expectation:
    // if global is unset, kzg_verify reports the missing-setup string;
    // if set (by another test), the input still must be 192 bytes.
    let bad_short = vec![0u8; 32];
    let res = zbx_zvm::precompiles::call_precompile(
        &zbx_zvm::precompiles::addresses::KZG_VERIFY,
        &bad_short,
        100_000,
    );
    assert!(res.is_err(), "short input must error one way or another");
}

#[test]
fn kzg_zvm_scalar_above_modulus_rejected() {
    let s = Scalar::random(&mut OsRng);
    let (settings, mut input) = make_valid(s);
    // Replace z with the modulus itself (not canonical: must be < r).
    input[32..64].copy_from_slice(&BLS_MODULUS_BE);
    // The versioned-hash check still passes (we only touched z), so the
    // failure surfaces at the scalar decode step.
    let res = do_kzg_with_settings(&input, 100_000, &settings);
    let msg = format!("{:?}", res.unwrap_err());
    assert!(msg.contains("canonical") || msg.contains("field"),
        "expected scalar-not-in-field, got: {msg}");
}

#[test]
fn kzg_zvm_setup_mismatch_rejects_otherwise_valid_proof() {
    // Build a valid proof under setup A; verify under setup B → must fail.
    let s_a = Scalar::random(&mut OsRng);
    let s_b = Scalar::random(&mut OsRng);
    let (settings_b, _) = test_setup(s_b);
    let (_, input) = make_valid(s_a);

    let res = do_kzg_with_settings(&input, 100_000, &settings_b);
    assert!(res.is_err(), "wrong-setup verification must fail: {:?}", res);
}

#[test]
fn kzg_zvm_crosscheck_lower_layer_agrees() {
    // Belt-and-suspenders: independently call the zbx-crypto core verifier
    // with the same settings/input and assert it agrees with the ZVM wrapper.
    let s = Scalar::random(&mut OsRng);
    let (settings, input) = make_valid(s);

    let zvm_res = do_kzg_with_settings(&input, 100_000, &settings);
    let core_res = do_kzg_point_eval(&input, 100_000, &settings);

    assert!(zvm_res.is_ok());
    assert!(core_res.is_ok());
    assert_eq!(zvm_res.unwrap().0, core_res.unwrap().0);
}

#[test]
fn kzg_zvm_versioned_hash_helper_is_self_consistent() {
    // Helper: kzg_to_versioned_hash always begins with 0x01 and is a
    // pure function of the commitment.
    let c1 = [1u8; 48];
    let c2 = [2u8; 48];
    let v1 = kzg_to_versioned_hash(&c1);
    let v2 = kzg_to_versioned_hash(&c2);
    assert_ne!(v1, v2);
    assert_eq!(v1[0], 0x01);
    assert_eq!(v2[0], 0x01);
    assert_eq!(kzg_to_versioned_hash(&c1), v1);
    let _ = KzgError::PairingFailed; // touch enum to keep import live
}
