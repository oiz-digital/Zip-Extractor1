//! SEC-2026-05-09 Pass-18 — BLS Proof-of-Possession at validator registration.
//!
//! Pre-Pass-18: `ValidatorSet::register` accepted any 48-byte `BlsPubKey`
//! without proving the registrant possessed the matching secret key. A
//! malicious validator could publish `pk_attacker = pk_real - sum(pk_others)`
//! and forge an aggregate-sig the verifier would accept as committee-signed.
//!
//! Pass-18 adds `register_with_pop` which verifies a BLS signature over
//! `keccak256(address || "zbx-bls-pop-v1")` before admission. Legacy
//! `register` is retained for genesis-only paths and explicitly documented
//! as PoP-bypass.

use rand::rngs::OsRng;
use zbx_crypto::bls::{BlsPrivKey, BlsSignature};
use zbx_crypto::keccak::keccak256;
use zbx_staking::error::StakingError;
use zbx_staking::validator::ValidatorSet;
use zbx_staking::MIN_SELF_STAKE;
use zbx_types::address::Address;
use zbx_types::H256;

/// Build a canonical PoP for a (validator_address, bls_sk) pair.
fn make_pop(addr: &Address, sk: &BlsPrivKey) -> BlsSignature {
    let mut preimg = Vec::with_capacity(20 + 14);
    preimg.extend_from_slice(addr.as_bytes());
    preimg.extend_from_slice(b"zbx-bls-pop-v1");
    let digest: H256 = keccak256(&preimg);
    sk.sign(&digest)
}

#[test]
fn register_with_pop_accepts_valid_proof() {
    let mut vs = ValidatorSet::new();
    let addr = Address([0xA1u8; 20]);
    let sk = BlsPrivKey::generate(&mut OsRng);
    let pk = sk.to_pubkey();
    let pop = make_pop(&addr, &sk);

    let res = vs.register_with_pop(addr, pk, pop, MIN_SELF_STAKE, 500);
    assert!(res.is_ok(), "valid PoP must be accepted: {res:?}");
    assert!(vs.get(&addr).is_some());
}

#[test]
fn register_with_pop_rejects_pop_signed_for_wrong_address() {
    let mut vs = ValidatorSet::new();
    let alice = Address([0x01u8; 20]);
    let mallory = Address([0x02u8; 20]);
    let sk = BlsPrivKey::generate(&mut OsRng);
    let pk = sk.to_pubkey();

    // Mallory presents a PoP that Alice's BLS key signed for *Alice's* address,
    // but tries to register it under Mallory's address. Must reject.
    let pop_for_alice = make_pop(&alice, &sk);
    let res = vs.register_with_pop(mallory, pk, pop_for_alice, MIN_SELF_STAKE, 500);
    assert!(matches!(res, Err(StakingError::InvalidEvidence(_))));
    assert!(vs.get(&mallory).is_none(), "Mallory must not be registered");
}

#[test]
fn register_with_pop_rejects_rogue_key_with_unknown_secret() {
    // Simulate a rogue-key attacker who publishes a `pk` they do NOT control
    // the secret for. They cannot produce a valid PoP, so registration fails.
    let mut vs = ValidatorSet::new();
    let attacker = Address([0xEEu8; 20]);

    let real_sk = BlsPrivKey::generate(&mut OsRng);
    let real_pk = real_sk.to_pubkey();

    // Attacker publishes a *different* secret's pubkey but signs with their own.
    let attacker_sk = BlsPrivKey::generate(&mut OsRng);
    let bad_pop = make_pop(&attacker, &attacker_sk);

    let res = vs.register_with_pop(attacker, real_pk, bad_pop, MIN_SELF_STAKE, 500);
    assert!(matches!(res, Err(StakingError::InvalidEvidence(_))));
}

#[test]
fn register_with_pop_rejects_random_garbage_signature() {
    let mut vs = ValidatorSet::new();
    let addr = Address([0xCCu8; 20]);
    let sk = BlsPrivKey::generate(&mut OsRng);
    let pk = sk.to_pubkey();

    // Signature bytes that aren't a valid G2 point would be rejected at
    // verify time (or if they happen to decode, the pairing check fails).
    let bad_pop = BlsSignature([0u8; 96]);
    let res = vs.register_with_pop(addr, pk, bad_pop, MIN_SELF_STAKE, 500);
    assert!(matches!(res, Err(StakingError::InvalidEvidence(_))));
}

#[test]
fn pop_verification_domain_is_canonical() {
    // Cross-check that BlsPubKey::verify_pop matches the PoP construction
    // in this test file — so a PoP produced anywhere in the workspace using
    // the documented domain `keccak256(addr || "zbx-bls-pop-v1")` verifies.
    let addr = Address([0x77u8; 20]);
    let sk = BlsPrivKey::generate(&mut OsRng);
    let pk = sk.to_pubkey();
    let pop = make_pop(&addr, &sk);
    assert!(pk.verify_pop(&pop, &addr));
    // Wrong address must not verify.
    assert!(!pk.verify_pop(&pop, &Address([0x78u8; 20])));
}

#[test]
fn legacy_register_is_pop_free_for_backward_compat() {
    // Genesis loaders that source keys from a trusted setup ceremony
    // continue to work via the legacy path. Documented as PoP-bypass.
    let mut vs = ValidatorSet::new();
    let addr = Address([0x33u8; 20]);
    let sk = BlsPrivKey::generate(&mut OsRng);
    let pk = sk.to_pubkey();
    let res = vs.register(addr, pk, MIN_SELF_STAKE, 500);
    assert!(res.is_ok());
}
