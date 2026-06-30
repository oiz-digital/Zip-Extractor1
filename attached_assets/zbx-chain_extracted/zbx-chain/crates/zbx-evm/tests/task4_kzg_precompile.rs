//! Task #4 (Precompile 0x0B — EIP-4844 KZG point evaluation): EVM-side tests
//! plus the cross-VM consensus-equivalence assertion.

use bls12_381::{G1Affine, G1Projective, G2Projective, Scalar};
use ff::Field;
use group::Curve;
use rand::rngs::OsRng;

use zbx_crypto::kzg::{
    do_kzg_point_eval, kzg_to_versioned_hash, point_evaluation_success_return,
    KzgSettings, BLS_MODULUS_BE,
};
use zbx_evm::precompiles::do_kzg_with_settings as evm_kzg;
use zbx_zvm::precompiles::do_kzg_with_settings as zvm_kzg;

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
    for (i, x) in le.iter().enumerate() { be[31 - i] = *x; }
    be
}

fn build_input(c: &G1Affine, z: &Scalar, y: &Scalar, pi: &G1Affine) -> Vec<u8> {
    let c_b = c.to_compressed();
    let pi_b = pi.to_compressed();
    let vh = kzg_to_versioned_hash(&c_b);
    let mut v = Vec::with_capacity(192);
    v.extend_from_slice(&vh);
    v.extend_from_slice(&scalar_to_be32(z));
    v.extend_from_slice(&scalar_to_be32(y));
    v.extend_from_slice(&c_b);
    v.extend_from_slice(&pi_b);
    v
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
fn kzg_evm_valid_proof_returns_success_constants() {
    let s = Scalar::random(&mut OsRng);
    let (settings, input) = make_valid(s);
    let (out, gas) = evm_kzg(&input, 100_000, &settings).expect("valid");
    assert_eq!(gas, 50_000);
    assert_eq!(out, point_evaluation_success_return());
    assert_eq!(&out[32..64], &BLS_MODULUS_BE);
}

#[test]
fn kzg_evm_wrong_versioned_hash_reverts() {
    let s = Scalar::random(&mut OsRng);
    let (settings, mut input) = make_valid(s);
    input[5] ^= 0xFF;
    assert!(evm_kzg(&input, 100_000, &settings).is_err());
}

#[test]
fn kzg_evm_tampered_proof_reverts() {
    let s = Scalar::random(&mut OsRng);
    let (settings, mut input) = make_valid(s);
    input[191] ^= 0x01;
    assert!(evm_kzg(&input, 100_000, &settings).is_err());
}

#[test]
fn kzg_evm_wrong_length_reverts() {
    let s = Scalar::random(&mut OsRng);
    let (settings, input) = make_valid(s);
    assert!(evm_kzg(&input[..191], 100_000, &settings).is_err());
    let mut long = input.clone();
    long.push(0u8);
    assert!(evm_kzg(&long, 100_000, &settings).is_err());
}

#[test]
fn kzg_evm_out_of_gas() {
    let s = Scalar::random(&mut OsRng);
    let (settings, input) = make_valid(s);
    let res = evm_kzg(&input, 49_999, &settings);
    assert!(matches!(res, Err(zbx_evm::error::EvmError::OutOfGas)),
        "expected OOG, got {:?}", res);
}

#[test]
fn kzg_evm_setup_mismatch_rejects_otherwise_valid_proof() {
    let s_a = Scalar::random(&mut OsRng);
    let s_b = Scalar::random(&mut OsRng);
    let (settings_b, _) = test_setup(s_b);
    let (_, input) = make_valid(s_a);
    assert!(evm_kzg(&input, 100_000, &settings_b).is_err());
}

#[test]
fn kzg_evm_dispatcher_recognizes_address_0x0b() {
    use zbx_types::address::Address;
    let mut a = [0u8; 20];
    a[19] = 0x0B;
    let addr = Address(a);
    assert!(zbx_evm::precompiles::is_precompile(&addr));
}

/// CROSS-VM CONSENSUS EQUIVALENCE: the same input + same trusted setup
/// must produce byte-identical (output, gas) tuples in both engines.
/// This is the consensus-critical guarantee — divergence here means a
/// chain split between EVM-deployed and ZVM-deployed contracts that
/// share the same blob attestation.
#[test]
fn kzg_cross_vm_consensus_byte_equivalence() {
    let s = Scalar::random(&mut OsRng);
    let (settings, input) = make_valid(s);

    let evm_ok = evm_kzg(&input, 100_000, &settings).expect("evm valid");
    let zvm_ok = zvm_kzg(&input, 100_000, &settings).expect("zvm valid");
    assert_eq!(evm_ok.0, zvm_ok.0, "valid: output bytes must match");
    assert_eq!(evm_ok.1, zvm_ok.1, "valid: gas used must match");

    // Tamper: same change observed by both engines must produce the
    // same fail-shape (Err with same root cause). We compare the
    // discriminant-free Err arm via .is_err(), since EVM and ZVM use
    // different error enums — the consensus-relevant guarantee is the
    // success vs. failure decision, not the human-readable message.
    let mut bad = input.clone();
    bad[5] ^= 0xFF;
    assert!(evm_kzg(&bad, 100_000, &settings).is_err());
    assert!(zvm_kzg(&bad, 100_000, &settings).is_err());

    let mut bad2 = input.clone();
    bad2[191] ^= 0x01;
    assert!(evm_kzg(&bad2, 100_000, &settings).is_err());
    assert!(zvm_kzg(&bad2, 100_000, &settings).is_err());

    // Insufficient gas: both engines must surface OOG.
    let evm_oog = evm_kzg(&input, 49_999, &settings);
    let zvm_oog = zvm_kzg(&input, 49_999, &settings);
    assert!(matches!(evm_oog, Err(zbx_evm::error::EvmError::OutOfGas)));
    assert!(matches!(zvm_oog, Err(zbx_zvm::error::ZvmError::OutOfGas)));

    // Last belt-and-suspenders: lower-layer agrees too.
    let core = do_kzg_point_eval(&input, 100_000, &settings).unwrap();
    assert_eq!(evm_ok.0, core.0);
    assert_eq!(zvm_ok.0, core.0);
}
