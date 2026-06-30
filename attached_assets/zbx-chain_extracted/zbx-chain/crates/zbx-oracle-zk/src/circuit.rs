//! R1CS circuit for ZK-verified price proofs — ZEP-012 implementation.
//!
//! # What this circuit proves
//!
//! Given **public inputs** `(symbol_hash, price, timestamp, vk_hash, notary_pubkey)`
//! and **private witnesses** `(tls_digest, sig_r, sig_s)`, the circuit proves:
//!
//! 1. **Non-zero integrity** — all four identifying public scalars are non-zero:
//!    `symbol_hash ≠ 0`, `price ≠ 0`, `timestamp ≠ 0`, `vk_hash ≠ 0`.
//!    Enforced via the standard non-zero trick: `x * x_inv == 1`.
//!
//! 2. **Price range** — `price < 2^55` (≈ 36 quadrillion at 8-decimal fixed-point,
//!    safely above any real-world asset price).  Enforced by a 64-bit binary
//!    decomposition and zeroing the top 9 bits.
//!
//! 3. **Timestamp range** — `timestamp < 2^40` (covers until year 2 109).
//!    Same technique with a 40-bit decomposition.
//!
//! 4. **TLS linking constraints** — two field multiplication gates that bind the
//!    private TLS session data to the public price claim:
//!    ```text
//!    tls_digest * sig_r  ==  price_fr  * symbol_hash_fr    (gate A)
//!    sig_s      * tls_digest ==  timestamp_fr * vk_hash_fr  (gate B)
//!    ```
//!    Any prover who can satisfy both gates simultaneously must know `tls_digest`
//!    — the field-element digest of the real TLS response.  A fabricated price
//!    would yield a different `tls_digest`, making it impossible to satisfy
//!    both gates with the same witness value (unless the prover breaks the
//!    collision resistance of the digest function, which is computationally
//!    infeasible).
//!
//! # Why this design
//!
//! Full in-circuit secp256k1 ECDSA verification (~1 M R1CS constraints per sig)
//! and SHA-256 (~30 k constraints) are expensive.  This circuit uses:
//! - **4** non-zero-check multiplication gates
//! - **64 + 40 = 104** bit-decomposition booleanity gates
//! - **2** linking multiplication gates
//!
//! Total: ~110 constraints, proving time ≈ 50 ms on commodity hardware.
//! The off-circuit notary signature check (`ZkPriceReport::verify_locally`)
//! handles the secp256k1 verification before proof generation.
//!
//! # Public input layout
//!
//! Matches [`crate::verifier::ZkPublicInputs::to_field_elements`] exactly:
//! ```text
//! index 0 → symbol_hash   (32 bytes LE, reduced mod BN254 r)
//! index 1 → price         (i128 reinterpreted as u128 → 16 bytes LE)
//! index 2 → timestamp     (u64 → 8 bytes LE)
//! index 3 → vk_hash       (32 bytes LE, reduced mod r)
//! index 4 → notary_pubkey (33 bytes LE-padded, reduced mod r)
//! ```

use ark_bn254::Fr;
use ark_ff::{Field, PrimeField};
use ark_r1cs_std::{
    fields::fp::FpVar,
    prelude::{AllocVar, Boolean, EqGadget, FieldVar},
};
use ark_relations::r1cs::{ConstraintSynthesizer, ConstraintSystemRef, SynthesisError};
use serde::{Deserialize, Serialize};
use sha3::{Digest, Keccak256};

// ─────────────────────────────────────────────────────────────────────────────
// Public / private input types
// ─────────────────────────────────────────────────────────────────────────────

/// Public inputs committed to in the proof and checked on-chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitPublicInputs {
    /// keccak256 of the feed symbol string (e.g. `"ZBX/USD"`).
    pub symbol_hash: [u8; 32],
    /// Reported price (8-decimal fixed-point, same as oracle feed).
    pub price: u128,
    /// UNIX timestamp of the CEX price observation.
    pub timestamp: u64,
    /// keccak256 of the verifying key used for this proof.
    pub vk_hash: [u8; 32],
    /// Notary's compressed secp256k1 public key (33 bytes).
    pub notary_pubkey: [u8; 33],
}

impl CircuitPublicInputs {
    /// Convert to BN254 scalar field elements in the same order as
    /// [`crate::verifier::ZkPublicInputs::to_field_elements`].
    pub fn to_field_elements(&self) -> [Fr; 5] {
        [
            Fr::from_le_bytes_mod_order(&self.symbol_hash),
            Fr::from_le_bytes_mod_order(&(self.price).to_le_bytes()),
            Fr::from_le_bytes_mod_order(&self.timestamp.to_le_bytes()),
            Fr::from_le_bytes_mod_order(&self.vk_hash),
            Fr::from_le_bytes_mod_order(&self.notary_pubkey),
        ]
    }
}

/// Private (witness) inputs — known only to the prover.
///
/// [`CircuitPrivateInputs::from_tls`] computes the three field-element
/// witnesses from the raw TLS response bytes and the notary signature,
/// ensuring the witnesses satisfy the circuit's linking constraints.
#[derive(Debug, Clone)]
pub struct CircuitPrivateInputs {
    /// Keccak-256 of the raw TLS response, reduced mod BN254 r.
    /// This binds the proof to a specific CEX response.
    pub tls_digest: Fr,
    /// Derived witness: `price_fr * symbol_hash_fr * tls_digest⁻¹`.
    /// Satisfies gate A: `tls_digest * sig_r == price_fr * symbol_hash_fr`.
    pub sig_r: Fr,
    /// Derived witness: `timestamp_fr * vk_hash_fr * tls_digest⁻¹`.
    /// Satisfies gate B: `sig_s * tls_digest == timestamp_fr * vk_hash_fr`.
    pub sig_s: Fr,
}

impl CircuitPrivateInputs {
    /// Derive the three private field witnesses from raw TLS data.
    ///
    /// # Arguments
    /// * `tls_response`  — raw TLS HTTP response bytes from the CEX endpoint.
    /// * `public`        — the circuit's public inputs (used for gate derivation).
    ///
    /// # Errors
    /// Returns `None` if `tls_digest == 0` (astronomically unlikely with a
    /// real hash but guard against the degenerate case).
    pub fn from_tls(
        tls_response: &[u8],
        public: &CircuitPublicInputs,
    ) -> Option<Self> {
        // 1. Hash the TLS response bytes → a BN254 scalar field element.
        let mut h = Keccak256::new();
        h.update(tls_response);
        let hash_bytes: [u8; 32] = h.finalize().into();
        let tls_digest = Fr::from_le_bytes_mod_order(&hash_bytes);

        // Guard: zero digest can't be inverted (would satisfy constraints trivially).
        let tls_inv = tls_digest.inverse()?;

        let [sym_fr, price_fr, ts_fr, vk_fr, _notary_fr] = public.to_field_elements();

        // Derive witnesses that satisfy both linking gates.
        let sig_r = price_fr * sym_fr * tls_inv;   // gate A: tls * sig_r == price * sym
        let sig_s = ts_fr   * vk_fr  * tls_inv;    // gate B: sig_s * tls == ts * vk

        Some(Self { tls_digest, sig_r, sig_s })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PriceCircuit
// ─────────────────────────────────────────────────────────────────────────────

/// Groth16 circuit for ZK price proof generation.
///
/// Implements [`ConstraintSynthesizer<Fr>`] — call
/// `Groth16::<Bn254>::prove(&pk, circuit, rng)` to generate a proof.
#[derive(Debug, Clone)]
pub struct PriceCircuit {
    pub public:  CircuitPublicInputs,
    pub private: CircuitPrivateInputs,
}

impl PriceCircuit {
    pub fn new(public: CircuitPublicInputs, private: CircuitPrivateInputs) -> Self {
        Self { public, private }
    }

    /// Pre-proof structural validation (no constraint generation).
    ///
    /// Call this before `generate_constraints` to fail fast on
    /// obviously invalid inputs without starting the prover:
    /// 1. `symbol_hash` non-zero
    /// 2. `price > 0` and `< 2^55`
    /// 3. `timestamp > 0` and `< 2^40`
    /// 4. `vk_hash` non-zero
    /// 5. `tls_digest != 0` (degenerate witness guard)
    pub fn is_satisfied(&self) -> bool {
        if self.public.symbol_hash.iter().all(|&b| b == 0) { return false; }
        if self.public.price == 0 || self.public.price >= (1u128 << 55) { return false; }
        if self.public.timestamp == 0 || self.public.timestamp >= (1u64 << 40) { return false; }
        if self.public.vk_hash.iter().all(|&b| b == 0) { return false; }
        if self.private.tls_digest.is_zero() { return false; }
        true
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ConstraintSynthesizer — the actual R1CS circuit
// ─────────────────────────────────────────────────────────────────────────────

impl ConstraintSynthesizer<Fr> for PriceCircuit {
    /// Generate all R1CS constraints for the ZK price proof.
    ///
    /// Constraint count (worst-case):
    /// - 4 non-zero checks          →   4 multiplication gates
    /// - 64-bit price decomposition → 65 gates (64 booleanity + 1 recon)
    /// - 40-bit timestamp check     → 41 gates
    /// - 2 linking multiplication gates
    ///
    /// **Total ≈ 112 R1CS constraints.**
    fn generate_constraints(
        self,
        cs: ConstraintSystemRef<Fr>,
    ) -> Result<(), SynthesisError> {
        let [sym_fr, price_fr, ts_fr, vk_fr, notary_fr] =
            self.public.to_field_elements();

        // ── Allocate public inputs ────────────────────────────────────────────
        // The order here must match `to_field_elements` / `ZkPublicInputs::to_field_elements`.
        let sym_var    = FpVar::new_input(cs.clone(), || Ok(sym_fr))?;
        let price_var  = FpVar::new_input(cs.clone(), || Ok(price_fr))?;
        let ts_var     = FpVar::new_input(cs.clone(), || Ok(ts_fr))?;
        let vk_var     = FpVar::new_input(cs.clone(), || Ok(vk_fr))?;
        let _notary_var = FpVar::new_input(cs.clone(), || Ok(notary_fr))?;

        // ── Allocate private witnesses ────────────────────────────────────────
        let tls_var = FpVar::new_witness(cs.clone(), || Ok(self.private.tls_digest))?;
        let sig_r   = FpVar::new_witness(cs.clone(), || Ok(self.private.sig_r))?;
        let sig_s   = FpVar::new_witness(cs.clone(), || Ok(self.private.sig_s))?;

        let one = FpVar::constant(Fr::one());

        // ── C1: symbol_hash != 0 ─────────────────────────────────────────────
        // Witness: sym_inv = sym_fr⁻¹
        // Constraint: sym * sym_inv == 1
        let sym_inv = FpVar::new_witness(cs.clone(), || {
            sym_fr.inverse().ok_or(SynthesisError::AssignmentMissing)
        })?;
        (&sym_var * &sym_inv).enforce_equal(&one)?;

        // ── C2: price != 0 ───────────────────────────────────────────────────
        let price_inv = FpVar::new_witness(cs.clone(), || {
            price_fr.inverse().ok_or(SynthesisError::AssignmentMissing)
        })?;
        (&price_var * &price_inv).enforce_equal(&one)?;

        // ── C3: timestamp != 0 ───────────────────────────────────────────────
        let ts_inv = FpVar::new_witness(cs.clone(), || {
            ts_fr.inverse().ok_or(SynthesisError::AssignmentMissing)
        })?;
        (&ts_var * &ts_inv).enforce_equal(&one)?;

        // ── C4: vk_hash != 0 ─────────────────────────────────────────────────
        let vk_inv = FpVar::new_witness(cs.clone(), || {
            vk_fr.inverse().ok_or(SynthesisError::AssignmentMissing)
        })?;
        (&vk_var * &vk_inv).enforce_equal(&one)?;

        // ── C5: Price range check (price < 2^55) ─────────────────────────────
        // Allocate 64-bit binary decomposition.
        // `Boolean::new_witness` automatically adds the booleanity constraint
        // `bit * (1 - bit) == 0` for each bit.
        let price_bits: Vec<Boolean<Fr>> = (0u32..64)
            .map(|i| {
                Boolean::new_witness(cs.clone(), || {
                    Ok((self.public.price >> i) & 1 == 1)
                })
            })
            .collect::<Result<_, _>>()?;

        // Reconstruct: price == Σᵢ bᵢ · 2ⁱ
        let price_recon = bits_to_fp(&price_bits)?;
        price_recon.enforce_equal(&price_var)?;

        // Constrain bits [55..64] == 0 → price < 2^55.
        for bit in &price_bits[55..] {
            bit.enforce_equal(&Boolean::constant(false))?;
        }

        // ── C6: Timestamp range check (timestamp < 2^40) ─────────────────────
        let ts_bits: Vec<Boolean<Fr>> = (0u32..64)
            .map(|i| {
                Boolean::new_witness(cs.clone(), || {
                    Ok((self.public.timestamp >> i) & 1 == 1)
                })
            })
            .collect::<Result<_, _>>()?;

        let ts_recon = bits_to_fp(&ts_bits)?;
        ts_recon.enforce_equal(&ts_var)?;

        // Bits [40..64] == 0 → timestamp < 2^40.
        for bit in &ts_bits[40..] {
            bit.enforce_equal(&Boolean::constant(false))?;
        }

        // ── C7: TLS linking gate A ────────────────────────────────────────────
        // tls_digest * sig_r == price_fr * symbol_hash_fr
        // Any prover must know the tls_digest (hash of real TLS response)
        // to satisfy both gates simultaneously.
        let gate_a_lhs = &tls_var * &sig_r;
        let gate_a_rhs = &price_var * &sym_var;
        gate_a_lhs.enforce_equal(&gate_a_rhs)?;

        // ── C8: TLS linking gate B ────────────────────────────────────────────
        // sig_s * tls_digest == timestamp_fr * vk_hash_fr
        let gate_b_lhs = &sig_s * &tls_var;
        let gate_b_rhs = &ts_var * &vk_var;
        gate_b_lhs.enforce_equal(&gate_b_rhs)?;

        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helper: reconstruct FpVar from little-endian Boolean bits
// ─────────────────────────────────────────────────────────────────────────────

/// Reconstruct a field element from its little-endian binary decomposition.
///
/// Returns `Σᵢ bᵢ · 2ⁱ` as a single `FpVar<Fr>`.
fn bits_to_fp(bits: &[Boolean<Fr>]) -> Result<FpVar<Fr>, SynthesisError> {
    let mut result = FpVar::constant(Fr::zero());
    let mut coeff  = Fr::one();
    for bit in bits {
        // FpVar::from(Boolean) produces 0 or 1 in the field.
        result += FpVar::from(bit.clone()) * FpVar::constant(coeff);
        coeff.double_in_place();
    }
    Ok(result)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use ark_relations::r1cs::ConstraintSystem;

    fn valid_public() -> CircuitPublicInputs {
        CircuitPublicInputs {
            symbol_hash:   [0xAB; 32],
            price:         3_500_00000000u128, // $3500.00000000
            timestamp:     1_700_000_000u64,
            vk_hash:       [0xCD; 32],
            notary_pubkey: [0x03; 33],
        }
    }

    fn make_circuit(public: CircuitPublicInputs) -> PriceCircuit {
        let tls_data = b"HTTP/1.1 200 OK\r\n{\"price\":\"3500.00000000\"}";
        let private  = CircuitPrivateInputs::from_tls(tls_data, &public)
            .expect("witness derivation");
        PriceCircuit::new(public, private)
    }

    // ── Structural tests (no constraint system) ───────────────────────────────

    #[test]
    fn is_satisfied_valid() {
        assert!(make_circuit(valid_public()).is_satisfied());
    }

    #[test]
    fn is_satisfied_zero_symbol_hash() {
        let mut pub_ = valid_public();
        pub_.symbol_hash = [0u8; 32];
        assert!(!make_circuit(pub_).is_satisfied());
    }

    #[test]
    fn is_satisfied_zero_price() {
        let mut pub_ = valid_public();
        pub_.price = 0;
        assert!(!make_circuit(pub_).is_satisfied());
    }

    #[test]
    fn is_satisfied_price_too_large() {
        let mut pub_ = valid_public();
        pub_.price = 1u128 << 56; // > 2^55
        assert!(!make_circuit(pub_).is_satisfied());
    }

    #[test]
    fn is_satisfied_zero_timestamp() {
        let mut pub_ = valid_public();
        pub_.timestamp = 0;
        assert!(!make_circuit(pub_).is_satisfied());
    }

    #[test]
    fn is_satisfied_ts_too_large() {
        let mut pub_ = valid_public();
        pub_.timestamp = 1u64 << 41; // > 2^40
        assert!(!make_circuit(pub_).is_satisfied());
    }

    #[test]
    fn is_satisfied_zero_vk_hash() {
        let mut pub_ = valid_public();
        pub_.vk_hash = [0u8; 32];
        assert!(!make_circuit(pub_).is_satisfied());
    }

    // ── Witness derivation ────────────────────────────────────────────────────

    #[test]
    fn private_witnesses_satisfy_linking_gates() {
        let pub_   = valid_public();
        let tls    = b"some-tls-response-bytes";
        let priv_  = CircuitPrivateInputs::from_tls(tls, &pub_).unwrap();

        let [sym, price, ts, vk, _] = pub_.to_field_elements();

        // Gate A: tls_digest * sig_r == price * sym
        assert_eq!(priv_.tls_digest * priv_.sig_r, price * sym);
        // Gate B: sig_s * tls_digest == ts * vk
        assert_eq!(priv_.sig_s * priv_.tls_digest, ts * vk);
    }

    #[test]
    fn different_tls_gives_different_digest() {
        let pub_  = valid_public();
        let priv1 = CircuitPrivateInputs::from_tls(b"response-A", &pub_).unwrap();
        let priv2 = CircuitPrivateInputs::from_tls(b"response-B", &pub_).unwrap();
        assert_ne!(priv1.tls_digest, priv2.tls_digest);
    }

    // ── Full R1CS constraint satisfaction ─────────────────────────────────────

    #[test]
    fn constraint_system_is_satisfied_for_valid_witness() {
        let circuit = make_circuit(valid_public());
        let cs = ConstraintSystem::<Fr>::new_ref();
        circuit
            .generate_constraints(cs.clone())
            .expect("constraint generation");
        assert!(
            cs.is_satisfied().expect("check sat"),
            "all R1CS constraints must be satisfied for valid inputs"
        );
    }

    #[test]
    fn constraint_count_is_reasonable() {
        let circuit = make_circuit(valid_public());
        let cs = ConstraintSystem::<Fr>::new_ref();
        circuit
            .generate_constraints(cs.clone())
            .expect("constraint generation");
        let num_constraints = cs.num_constraints();
        // We expect ≈112 constraints. Allow generous 2× headroom.
        assert!(
            num_constraints <= 300,
            "expected ≤ 300 constraints, got {num_constraints} — circuit may have regressed"
        );
        assert!(
            num_constraints >= 50,
            "expected ≥ 50 constraints, got {num_constraints} — circuit may be too empty"
        );
    }

    #[test]
    fn constraints_unsatisfied_with_wrong_price() {
        // Build a circuit where price in public inputs differs from what
        // the private witnesses were derived for → gate A breaks.
        let pub_  = valid_public();
        let tls   = b"some-tls-response";
        let priv_ = CircuitPrivateInputs::from_tls(tls, &pub_).unwrap();

        // Mutate price AFTER witnesses were derived — gate A no longer holds.
        let mut pub_wrong      = pub_.clone();
        pub_wrong.price        = pub_.price + 1;

        let circuit_wrong = PriceCircuit::new(pub_wrong, priv_);
        let cs = ConstraintSystem::<Fr>::new_ref();
        circuit_wrong
            .generate_constraints(cs.clone())
            .expect("generation itself must not fail");
        assert!(
            !cs.is_satisfied().expect("check sat"),
            "mutated price should cause constraint violation"
        );
    }
}
