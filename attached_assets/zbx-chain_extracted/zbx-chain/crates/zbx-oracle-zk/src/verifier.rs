//! Off-chain ZK price-proof verifier — **real Groth16 over BN254**.
//!
//! ## Pass-17 (SEC-2026-05-09) — real verifier
//!
//! Pre-Pass-17 this module shipped a `verify()` body that was either:
//!
//!   * the original `Ok(price > 0)` stub (Pass-12 audit CRIT — accepted any
//!     proof for any positive price), OR
//!   * the Pass-12 fail-closed `Err(VerifierError::NotImplemented)` (safe but
//!     blocks the entire ZK-oracle pipeline from production use).
//!
//! Pass-17 wires it to the `arkworks` Groth16 implementation over BN254 —
//! the same curve as Ethereum's `ecAdd` / `ecMul` / `ecPairing` precompiles
//! (EIP-196 / EIP-197) so any proof verified here can be re-verified on-chain
//! by `contracts/ZbxGroth16Verifier.sol`. The same primitive backs
//! [`zbx_zk::verifier::Groth16Verifier`].
//!
//! ## Wire format
//!
//! * **Verifying key** is the canonical `arkworks` *compressed* serialisation
//!   of `ark_groth16::VerifyingKey<Bn254>` (`Compress::Yes`, `Validate::Yes`).
//! * **Proof** is the *uncompressed* 256-byte flat layout
//!   `pi_a (G1, 64 B) ‖ pi_b (G2, 128 B) ‖ pi_c (G1, 64 B)` produced by
//!   [`ZkPriceProof::proof_bytes_canonical`] and consumed via
//!   `ark_groth16::Proof::<Bn254>::deserialize_with_mode(.., Compress::No, ..)`.
//!
//! These two encodings intentionally differ: the VK is shipped once and
//! benefits from the smaller compressed form, while the per-block proof
//! comes from the existing flat-bytes wire format that on-chain Solidity
//! verifiers also consume.
//!
//! Public inputs are hashed from the [`ZkPublicInputs`] struct via
//! [`ZkPublicInputs::to_field_elements`]: each scalar field is reduced
//! mod the BN254 scalar prime. Whoever generates the proving key MUST
//! use the same circuit-side public-input layout (5 Fr scalars, in the
//! order: `symbol_hash`, `price`, `timestamp`, `vk_hash`, `notary_pubkey`).

use crate::proof::{ZkPriceProof, ZkPublicInputs};
use std::collections::HashMap;

use ark_bn254::{Bn254, Fr};
use ark_ff::PrimeField;
use ark_groth16::{Groth16, Proof as ArkProof, VerifyingKey as ArkVk, PreparedVerifyingKey};
use ark_serialize::{CanonicalDeserialize, Compress, Validate};
use ark_snark::SNARK;

// ─────────────────────────────────────────────────────────────────────────────
// Errors
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum VerifierError {
    #[error("unknown feed: {0}")]
    UnknownFeed(String),

    #[error("verifying key hash mismatch")]
    VkMismatch,

    #[error("invalid verifying-key bytes: {0}")]
    InvalidVkBytes(String),

    #[error("invalid proof encoding: {0}")]
    InvalidEncoding(String),

    #[error("pairing check failed")]
    PairingFailed,

    #[error("negative prices are not supported by the oracle ZK circuit")]
    NegativePrice,
}

// ─────────────────────────────────────────────────────────────────────────────
// Verifying key
// ─────────────────────────────────────────────────────────────────────────────

/// A Groth16 verifying key for one price feed (e.g. ZBX/USD).
///
/// `vk_bytes` are the canonical `arkworks` compressed serialisation of
/// `ark_groth16::VerifyingKey<Bn254>`. `vk_hash` is what the on-chain
/// circuit and the proof's [`ZkPublicInputs::vk_hash`] commit to —
/// typically `keccak256(vk_bytes)` (binding the verifier to the exact
/// trusted-setup output) but the chain only treats it as an opaque
/// 32-byte tag.
#[derive(Clone, Debug)]
pub struct ZkVerifyingKey {
    pub vk_bytes: Vec<u8>,
    pub vk_hash:  [u8; 32],
}

/// A pre-processed verifier holding the deserialized + prepared VK.
struct PreparedFeed {
    pvk:     PreparedVerifyingKey<Bn254>,
    vk_hash: [u8; 32],
}

pub struct ZkOracleVerifier {
    feeds: HashMap<String, PreparedFeed>,
}

impl Default for ZkOracleVerifier {
    fn default() -> Self { Self::new() }
}

impl ZkOracleVerifier {
    pub fn new() -> Self {
        Self { feeds: HashMap::new() }
    }

    /// Register a verifying key for a feed.
    ///
    /// Validates the VK bytes eagerly — bad bytes are rejected here rather
    /// than silently failing every later `verify()`.
    pub fn register_vk(&mut self, feed: String, vk: ZkVerifyingKey) -> Result<(), VerifierError> {
        let ark_vk = ArkVk::<Bn254>::deserialize_with_mode(
            vk.vk_bytes.as_slice(),
            Compress::Yes,
            Validate::Yes,
        )
        .map_err(|e| VerifierError::InvalidVkBytes(e.to_string()))?;
        let pvk = Groth16::<Bn254>::process_vk(&ark_vk)
            .map_err(|e| VerifierError::InvalidVkBytes(e.to_string()))?;
        self.feeds.insert(feed, PreparedFeed { pvk, vk_hash: vk.vk_hash });
        Ok(())
    }

    /// Verify a ZK price proof for the given feed.
    ///
    /// Steps:
    ///   1. Look up the prepared verifying key for this feed.
    ///   2. Re-check that the proof's public-input `vk_hash` matches the
    ///      registered VK's hash (binds the proof to a specific CEX/circuit).
    ///   3. Deserialize the proof bytes as `ark_groth16::Proof<Bn254>`.
    ///   4. Build the public-input field-element vector via
    ///      [`ZkPublicInputs::to_field_elements`].
    ///   5. Run the real BN254 pairing check via `Groth16::verify_with_processed_vk`.
    ///
    /// Returns `Ok(true)` only on a successful pairing check; otherwise
    /// `Ok(false)` for "well-formed but invalid" proofs and `Err(...)` for
    /// malformed inputs.
    pub fn verify(&self, proof: &ZkPriceProof, feed: &str) -> Result<bool, VerifierError> {
        let prepared = self.feeds.get(feed)
            .ok_or_else(|| VerifierError::UnknownFeed(feed.to_string()))?;

        if proof.public_inputs.vk_hash != prepared.vk_hash {
            return Err(VerifierError::VkMismatch);
        }

        // Pass-17 architect-review fix: i128 → u128 reinterpretation has
        // undefined cross-tooling semantics for negative values (circom and
        // snarkjs default to unsigned field elements). Reject negative
        // prices at the verifier boundary so the public-input packing has
        // exactly one well-defined representation.
        if proof.public_inputs.price < 0 {
            return Err(VerifierError::NegativePrice);
        }

        let proof_bytes = proof.proof_bytes_canonical();
        let ark_proof = ArkProof::<Bn254>::deserialize_with_mode(
            proof_bytes.as_slice(),
            Compress::No,    // legacy flat layout = uncompressed-style 64+128+64 bytes
            Validate::Yes,
        )
        .map_err(|e| VerifierError::InvalidEncoding(e.to_string()))?;

        let public_inputs = proof.public_inputs.to_field_elements();

        Groth16::<Bn254>::verify_with_processed_vk(
            &prepared.pvk,
            &public_inputs,
            &ark_proof,
        )
        .map_err(|_| VerifierError::PairingFailed)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Public-input layout
// ─────────────────────────────────────────────────────────────────────────────

impl ZkPublicInputs {
    /// Convert this struct into the BN254 Fr scalar vector that the
    /// circuit's public-input wire-up expects, in this fixed order:
    ///
    /// 1. `symbol_hash`   — 32 bytes, reduced mod r (LE)
    /// 2. `price`         — i128 reinterpreted as u128 LE-padded to 32 bytes
    /// 3. `timestamp`     — u64 LE-padded to 32 bytes
    /// 4. `vk_hash`       — 32 bytes, reduced mod r (LE)
    /// 5. `notary_pubkey` — 33 bytes, padded with one zero byte then reduced
    ///
    /// The proving-side circuit MUST use the exact same packing.
    pub fn to_field_elements(&self) -> Vec<Fr> {
        let mut out = Vec::with_capacity(5);

        out.push(Fr::from_le_bytes_mod_order(&self.symbol_hash));

        // i128 → 16 LE bytes (sign-extended via two's complement at the i128
        // level; the resulting unsigned u128 is what the circuit sees).
        let price_bytes = (self.price as u128).to_le_bytes();
        out.push(Fr::from_le_bytes_mod_order(&price_bytes));

        let ts_bytes = self.timestamp.to_le_bytes();
        out.push(Fr::from_le_bytes_mod_order(&ts_bytes));

        out.push(Fr::from_le_bytes_mod_order(&self.vk_hash));

        // 33-byte compressed secp256k1 pubkey reduced mod the BN254 scalar
        // field. This is a one-way commitment good enough for circuit
        // binding; it is NOT a secp256k1 point check (that happens in the
        // notary-attestation pre-check).
        out.push(Fr::from_le_bytes_mod_order(&self.notary_pubkey));

        out
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proof::ZkPriceProof;

    fn dummy_inputs() -> ZkPublicInputs {
        ZkPublicInputs {
            symbol_hash:   [1u8; 32],
            price:         3500_00000000,
            timestamp:     1_700_000_000,
            vk_hash:       [2u8; 32],
            notary_pubkey: [3u8; 33],
        }
    }

    fn dummy_proof(vk_hash: [u8; 32]) -> ZkPriceProof {
        let mut pub_in = dummy_inputs();
        pub_in.vk_hash = vk_hash;
        ZkPriceProof {
            groth16_a: [0u8; 64],
            groth16_b: [0u8; 128],
            groth16_c: [0u8; 64],
            public_inputs: pub_in,
        }
    }

    #[test]
    fn unknown_feed_rejects() {
        let v = ZkOracleVerifier::new();
        let p = dummy_proof([0u8; 32]);
        assert!(matches!(v.verify(&p, "ZBX/USD"), Err(VerifierError::UnknownFeed(_))));
    }

    #[test]
    fn register_garbage_vk_is_rejected() {
        let mut v = ZkOracleVerifier::new();
        let bad = ZkVerifyingKey { vk_bytes: vec![0xffu8; 100], vk_hash: [0u8; 32] };
        assert!(matches!(
            v.register_vk("ZBX/USD".into(), bad),
            Err(VerifierError::InvalidVkBytes(_))
        ));
    }

    #[test]
    fn empty_vk_bytes_rejected() {
        let mut v = ZkOracleVerifier::new();
        let bad = ZkVerifyingKey { vk_bytes: vec![], vk_hash: [0u8; 32] };
        assert!(matches!(
            v.register_vk("ZBX/USD".into(), bad),
            Err(VerifierError::InvalidVkBytes(_))
        ));
    }

    #[test]
    fn public_inputs_serialization_is_deterministic_and_distinct() {
        let mut a = dummy_inputs();
        let fa1 = a.to_field_elements();
        let fa2 = a.to_field_elements();
        assert_eq!(fa1, fa2);
        // Mutating any field changes the resulting Fr vector.
        a.price += 1;
        let fb = a.to_field_elements();
        assert_ne!(fa1, fb);
    }

    #[test]
    fn public_inputs_have_five_field_elements() {
        assert_eq!(dummy_inputs().to_field_elements().len(), 5);
    }

    #[test]
    fn negative_price_rejected_eagerly() {
        // Architect-review tightening: i128 → u128 reinterpretation has
        // undefined semantics across tooling, so we just reject.
        let mut v = ZkOracleVerifier::new();
        // Forge a minimal valid VK by round-tripping zero-knowledge
        // through arkworks — actually we don't need a real VK, because
        // negative-price rejection happens *after* the VK lookup but
        // *before* deserialisation. So we register a feed with a hash
        // that matches our proof's vk_hash and use a clearly bad VK
        // (which would fail later in the pipeline) — the negative-price
        // check should fire first.
        //
        // Simpler: just check that negative price short-circuits without
        // touching the VK at all by registering nothing and confirming
        // the UnknownFeed error fires; then by registering a bad VK and
        // confirming we never reach the proof-decode step. But the
        // current implementation orders: lookup → vk-check → negative-price
        // → decode. So we need a registered VK whose hash matches.
        //
        // Build the smallest valid VK we can: an empty (alpha=beta=gamma=
        // delta=0, ic=[]) arkworks VK. arkworks accepts this as
        // structurally valid bytes but a real proof would never verify
        // against it. We need this only to reach the negative-price guard.
        use ark_bn254::{G1Affine, G2Affine};
        use ark_groth16::VerifyingKey as ArkVk;
        use ark_serialize::CanonicalSerialize;
        let bogus_vk = ArkVk::<Bn254> {
            alpha_g1: G1Affine::default(),
            beta_g2:  G2Affine::default(),
            gamma_g2: G2Affine::default(),
            delta_g2: G2Affine::default(),
            gamma_abc_g1: vec![G1Affine::default(); 6], // 5 public inputs + 1
        };
        let mut buf = Vec::new();
        bogus_vk.serialize_compressed(&mut buf).unwrap();
        let vk_hash = [9u8; 32];
        v.register_vk("FOO".into(), ZkVerifyingKey { vk_bytes: buf, vk_hash }).unwrap();
        let mut p = dummy_proof(vk_hash);
        p.public_inputs.price = -1;
        assert!(matches!(v.verify(&p, "FOO"), Err(VerifierError::NegativePrice)));
    }
}
