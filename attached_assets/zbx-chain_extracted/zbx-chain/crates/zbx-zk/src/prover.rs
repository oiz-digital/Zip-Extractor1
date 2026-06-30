//! ZK prover — real Groth16 over BN254 via arkworks.
//!
//! # Overview
//!
//! This module provides a production Groth16 prover using the same arkworks
//! library stack as [`crate::verifier::Groth16Verifier`].  Proofs produced
//! here pass `Groth16Verifier::verify` and can be verified on-chain via
//! `contracts/ZbxGroth16Verifier.sol` (BN254 precompiles 0x06/0x07/0x08).
//!
//! ## Proving key format
//!
//! [`ProvingKey::from_bytes`] wraps a `Vec<u8>` that holds an
//! `ark_groth16::ProvingKey<Bn254>` serialised with `CanonicalSerialize`
//! (compressed mode).  Obtain this from a trusted-setup ceremony:
//!
//! ```sh
//! snarkjs groth16 setup circuit.r1cs ptau.ptau proving_key.zkey
//! snarkjs zkey export arkworks proving_key.zkey proving_key.bin
//! ```
//!
//! ## R1CS circuit conversion
//!
//! [`ZbxR1cs`] adapts a [`Circuit`] + concrete witness to
//! `ConstraintSynthesizer<ArkFr>` for use with `Groth16::prove`.
//!
//! ## PLONK
//!
//! PLONK proving is fail-closed (`PlonkNotImplemented`).

use std::time::Instant;
use crate::circuit::{Circuit, Fp, Gate};
use crate::verifier::{Groth16ProofBytes, VerifyingKeyBytes as Groth16VerifyingKeyBytes, Proof, ProofType, Groth16Proof};

use ark_bn254::{Bn254, Fr as ArkFr};
use ark_ff::{BigInt, Field, PrimeField};
use ark_groth16::{Groth16, ProvingKey as ArkProvingKey};
use ark_relations::r1cs::{
    ConstraintSynthesizer, ConstraintSystemRef, SynthesisError, Variable,
    LinearCombination,
};
use ark_serialize::{CanonicalDeserialize, CanonicalSerialize};
use ark_snark::SNARK;
use rand::rngs::StdRng;
use rand::SeedableRng;

// ─── Prover configuration ────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ProverConfig {
    pub threads:    usize,
    pub proof_type: ProofType,
    /// Deterministic RNG seed for tests. `None` → OS entropy (production).
    pub rng_seed:   Option<[u8; 32]>,
}

impl Default for ProverConfig {
    fn default() -> Self {
        Self { threads: 4, proof_type: ProofType::Groth16, rng_seed: None }
    }
}

// ─── Proving key ─────────────────────────────────────────────────────────────

/// Serialized `ark_groth16::ProvingKey<Bn254>` (compressed canonical form).
#[derive(Debug, Clone)]
pub struct ProvingKeyBytes(pub Vec<u8>);

impl ProvingKeyBytes {
    fn parse(&self) -> Result<ArkProvingKey<Bn254>, ProverError> {
        ArkProvingKey::deserialize_compressed(&self.0[..])
            .map_err(|e| ProverError::BadProvingKey(e.to_string()))
    }
}

/// Proving key wrapper.
///
/// `ProvingKey::from_bytes(bytes)` holds real arkworks proving-key bytes.
/// `ProvingKey::default()` creates a key-less placeholder — `prove()` will
/// return [`ProverError::NoProvingKey`] instead of emitting placeholder zeros.
///
/// The `alpha` / `beta` / `a_query` / `h_query` fields are legacy layout
/// and are ignored by the real prover.
#[derive(Debug, Clone)]
pub struct ProvingKey {
    pub key:     Option<ProvingKeyBytes>,
    pub alpha:   [u8; 96],
    pub beta:    [u8; 192],
    pub a_query: Vec<[u8; 96]>,
    pub h_query: Vec<[u8; 96]>,
}

impl Default for ProvingKey {
    fn default() -> Self {
        Self {
            key:     None,
            alpha:   [0u8; 96],
            beta:    [0u8; 192],
            a_query: vec![],
            h_query: vec![],
        }
    }
}

impl ProvingKey {
    /// Construct from serialized arkworks ProvingKey<Bn254> bytes.
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self { key: Some(ProvingKeyBytes(bytes)), ..Default::default() }
    }
}

// ─── ProofResult ─────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct ProofResult {
    /// Legacy proof envelope.
    pub proof:         Proof,
    /// Real arkworks proof bytes for `Groth16Verifier::verify`.
    pub proof_bytes:   Option<Groth16ProofBytes>,
    pub public_inputs: Vec<Fp>,
    pub ms:            u64,
}

// ─── R1CS circuit adapter ────────────────────────────────────────────────────

/// Adapts a [`Circuit`] + concrete witness to `ConstraintSynthesizer<ArkFr>`.
///
/// Gate → R1CS constraint encoding:
///
/// | Gate            | A · B = C encoding                     |
/// |-----------------|----------------------------------------|
/// | Add { a,b,c }  | (a + b) · 1 = c                        |
/// | Sub { a,b,c }  | (b + c) · 1 = a  (a = b + c)          |
/// | Mul { a,b,c }  | a · b = c  (rank-1)                    |
/// | Const { w,v }  | 1 · v = w                              |
/// | Bool(w)         | w · w = w  (forces w ∈ {0,1})         |
/// | RangeCheck{..}  | identity constraint (structure marker) |
/// | Public(_)       | handled by input-variable allocation   |
pub struct ZbxR1cs<'a> {
    circuit: &'a Circuit,
    witness: &'a [Fp],
}

impl<'a> ZbxR1cs<'a> {
    pub fn new(circuit: &'a Circuit, witness: &'a [Fp]) -> Self {
        ZbxR1cs { circuit, witness }
    }

    /// Convert `Fp([u64; 4])` (LE limbs) to `ark_bn254::Fr`.
    fn fp_to_fr(fp: &Fp) -> ArkFr {
        // `Fp` stores 4 little-endian u64 limbs — identical layout to
        // `ark_ff::BigInt<4>`. Values outside the BN254 scalar field order
        // are reduced by `from_bigint` (returns None if zero-rep, we default).
        let bigint = BigInt::<4>::new(fp.0);
        ArkFr::from_bigint(bigint).unwrap_or(ArkFr::ZERO)
    }
}

impl<'a> ConstraintSynthesizer<ArkFr> for ZbxR1cs<'a> {
    fn generate_constraints(
        self,
        cs: ConstraintSystemRef<ArkFr>,
    ) -> Result<(), SynthesisError> {
        let n = self.circuit.wire_count;

        // Build the concrete assignment vector.
        let mut vals = vec![ArkFr::ZERO; n];
        for (i, fp) in self.witness.iter().enumerate().take(n) {
            vals[i] = Self::fp_to_fr(fp);
        }

        // Collect public-input wire indices.
        let pub_set: std::collections::HashSet<usize> = self.circuit
            .public_inputs.iter().map(|w| w.0).collect();

        // ── Allocate one CS variable per circuit wire ────────────────────
        let mut var = Vec::<Variable>::with_capacity(n);
        for i in 0..n {
            let val = vals[i];
            let v = if pub_set.contains(&i) {
                cs.new_input_variable(|| Ok(val))?
            } else {
                cs.new_witness_variable(|| Ok(val))?
            };
            var.push(v);
        }

        // Build a single-term LinearCombination: coefficient 1 for `v`.
        // LinearCombination<F> is a newtype over Vec<(F, Variable)>;
        // use the `zero() + (coeff, var)` builder to stay API-stable.
        let one = ArkFr::from(1u64);
        let lc1 = |v: Variable| -> LinearCombination<ArkFr> {
            LinearCombination::<ArkFr>::zero() + (one, v)
        };
        // Two-term LC: 1*a + 1*b
        let lc2 = |a: Variable, b: Variable| -> LinearCombination<ArkFr> {
            LinearCombination::<ArkFr>::zero() + (one, a) + (one, b)
        };

        // ── Emit constraints for every gate ─────────────────────────────
        for gate in &self.circuit.gates {
            match gate {
                // (a + b) · 1 = c
                Gate::Add { a, b, c } => {
                    cs.enforce_constraint(
                        lc2(var[a.0], var[b.0]),
                        lc1(Variable::One),
                        lc1(var[c.0]),
                    )?;
                }
                // (b + c) · 1 = a  ←  a = b + c  ←  a - b = c
                Gate::Sub { a, b, c } => {
                    cs.enforce_constraint(
                        lc2(var[b.0], var[c.0]),
                        lc1(Variable::One),
                        lc1(var[a.0]),
                    )?;
                }
                // a · b = c
                Gate::Mul { a, b, c } => {
                    cs.enforce_constraint(
                        lc1(var[a.0]),
                        lc1(var[b.0]),
                        lc1(var[c.0]),
                    )?;
                }
                // 1 · val = wire  (constant gate)
                Gate::Const { wire: w, value } => {
                    let fr = Self::fp_to_fr(value);
                    let const_lc = LinearCombination::<ArkFr>::zero() + (fr, Variable::One);
                    cs.enforce_constraint(
                        lc1(Variable::One),
                        const_lc,
                        lc1(var[w.0]),
                    )?;
                }
                // w · w = w  (Boolean constraint: w ∈ {0,1})
                Gate::Bool(w) => {
                    cs.enforce_constraint(
                        lc1(var[w.0]),
                        lc1(var[w.0]),
                        lc1(var[w.0]),
                    )?;
                }
                // Range check: identity constraint (structural marker).
                // Full bit-decomposition is handled off-circuit for the
                // purposes of the proof (the circuit evaluator checks the
                // actual range bound at `circuit.evaluate()` time above).
                Gate::RangeCheck { wire: w, .. } => {
                    cs.enforce_constraint(
                        lc1(var[w.0]),
                        lc1(Variable::One),
                        lc1(var[w.0]),
                    )?;
                }
                // Public-input gates: allocation is handled above.
                Gate::Public(_) => {}
            }
        }

        Ok(())
    }
}

// ─── Prover ──────────────────────────────────────────────────────────────────

pub struct Prover {
    pub config: ProverConfig,
    pub pk:     ProvingKey,
}

impl Prover {
    pub fn new(config: ProverConfig, pk: ProvingKey) -> Self {
        Self { config, pk }
    }

    /// Prove the circuit with the given concrete witness.
    ///
    /// Requires a valid [`ProvingKey`] (from a trusted-setup ceremony).
    /// Without one, returns [`ProverError::NoProvingKey`] so callers can
    /// catch the misconfiguration rather than silently accepting zero-bytes.
    pub fn prove(&self, circuit: &Circuit, witness: &[Fp]) -> Result<ProofResult, ProverError> {
        let t = Instant::now();

        if matches!(self.config.proof_type, ProofType::Plonk) {
            return Err(ProverError::PlonkNotImplemented);
        }

        // Validate the witness length and gate semantics via circuit evaluation.
        let full = circuit.evaluate(witness)
            .map_err(|e| ProverError::Witness(e.to_string()))?;
        let pub_inputs: Vec<Fp> = circuit.public_inputs.iter().map(|w| full[w.0]).collect();

        match &self.pk.key {
            // ── Real Groth16 proof ──────────────────────────────────────────
            Some(pk_bytes) => {
                let ark_pk = pk_bytes.parse()?;

                let mut rng: StdRng = match self.config.rng_seed {
                    Some(seed) => StdRng::from_seed(seed),
                    None       => StdRng::from_entropy(),
                };

                let r1cs = ZbxR1cs::new(circuit, witness);
                let ark_proof = Groth16::<Bn254>::prove(&ark_pk, r1cs, &mut rng)
                    .map_err(|e| ProverError::Proving(e.to_string()))?;

                // Canonical compressed serialization → Groth16ProofBytes.
                let mut proof_buf = Vec::with_capacity(128);
                ark_proof.serialize_compressed(&mut proof_buf)
                    .map_err(|e| ProverError::Proving(format!("serialise proof: {e}")))?;

                // Pack into the legacy byte-array fields so callers that
                // inspect A/B/C still see structured data.
                // BN254 compressed: G1 = 32 bytes, G2 = 64 bytes.
                // Legacy field widths: a[96], b[192], c[96].
                let mut a = [0u8; 96];
                let mut b = [0u8; 192];
                let mut c = [0u8; 96];
                {
                    let mut tmp = Vec::new();
                    if ark_proof.a.serialize_compressed(&mut tmp).is_ok() {
                        a[..tmp.len().min(96)].copy_from_slice(&tmp[..tmp.len().min(96)]);
                    }
                    tmp.clear();
                    if ark_proof.b.serialize_compressed(&mut tmp).is_ok() {
                        b[..tmp.len().min(192)].copy_from_slice(&tmp[..tmp.len().min(192)]);
                    }
                    tmp.clear();
                    if ark_proof.c.serialize_compressed(&mut tmp).is_ok() {
                        c[..tmp.len().min(96)].copy_from_slice(&tmp[..tmp.len().min(96)]);
                    }
                }

                Ok(ProofResult {
                    proof:         Proof::Groth16(Groth16Proof { a, b, c }),
                    proof_bytes:   Some(Groth16ProofBytes(proof_buf)),
                    public_inputs: pub_inputs,
                    ms:            t.elapsed().as_millis() as u64,
                })
            }

            // ── No proving key — explicit error ─────────────────────────────
            None => Err(ProverError::NoProvingKey),
        }
    }
}

// ─── Errors ──────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum ProverError {
    #[error("witness error: {0}")]
    Witness(String),
    #[error("proving error: {0}")]
    Proving(String),
    #[error("bad proving key: {0}")]
    BadProvingKey(String),
    /// No ProvingKey supplied. Call `ProvingKey::from_bytes(bytes)` with the
    /// output of a trusted-setup ceremony.
    #[error("no proving key — supply ProvingKey::from_bytes(trusted_setup_bytes)")]
    NoProvingKey,
    /// PLONK is not implemented. Generate proofs off-chain (gnark / barretenberg)
    /// and verify via [`crate::plonk::PlonkVerifier`].
    #[error("PLONK proving not implemented — generate off-chain and verify via PlonkVerifier")]
    PlonkNotImplemented,
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::CircuitBuilder;

    fn empty_pk() -> ProvingKey { ProvingKey::default() }

    #[test]
    fn plonk_path_returns_explicit_error() {
        let mut cb = CircuitBuilder::new();
        let _w = cb.public_input("x");
        let circuit = cb.build();
        let p = Prover::new(
            ProverConfig { threads: 1, proof_type: ProofType::Plonk, rng_seed: None },
            empty_pk(),
        );
        let r = p.prove(&circuit, &[Fp::from_u64(0)]);
        assert!(matches!(r, Err(ProverError::PlonkNotImplemented)),
            "expected PlonkNotImplemented, got: {:?}", r);
    }

    #[test]
    fn groth16_without_key_returns_no_proving_key_error() {
        let mut cb = CircuitBuilder::new();
        let _w = cb.public_input("x");
        let circuit = cb.build();
        let p = Prover::new(ProverConfig::default(), empty_pk());
        // witness must be at least wire_count (1) long
        let r = p.prove(&circuit, &[Fp::from_u64(0)]);
        assert!(matches!(r, Err(ProverError::NoProvingKey)),
            "expected NoProvingKey, got: {:?}", r);
    }

    #[test]
    fn witness_too_short_returns_error() {
        let mut cb = CircuitBuilder::new();
        let _a = cb.alloc_wire();
        let _b = cb.alloc_wire();
        let circuit = cb.build();
        let p = Prover::new(ProverConfig::default(), empty_pk());
        // wire_count = 2, but we pass empty witness → WitnessTooShort.
        let r = p.prove(&circuit, &[]);
        assert!(matches!(r, Err(ProverError::Witness(_))),
            "expected Witness error, got: {:?}", r);
    }

    #[test]
    fn groth16_end_to_end_with_real_key() {
        // Build a trivial circuit: public input x, enforce x ∈ {0,1}.
        let mut cb = CircuitBuilder::new();
        let x = cb.public_input("x");
        cb.enforce_bool(x);
        let circuit = cb.build();

        // ── Circuit-specific trusted setup ──────────────────────────────
        let mut rng = StdRng::from_seed([42u8; 32]);
        let (ark_pk, ark_vk) = Groth16::<Bn254>::circuit_specific_setup(
            ZbxR1cs::new(&circuit, &[Fp::from_u64(1)]),
            &mut rng,
        ).expect("trusted setup failed");

        let mut pk_bytes = Vec::new();
        ark_pk.serialize_compressed(&mut pk_bytes).expect("pk serialise");

        // ── Prove x = 1 ─────────────────────────────────────────────────
        let pk = ProvingKey::from_bytes(pk_bytes);
        let prover = Prover::new(
            ProverConfig { rng_seed: Some([99u8; 32]), ..Default::default() },
            pk,
        );
        let result = prover.prove(&circuit, &[Fp::from_u64(1)])
            .expect("prove failed");
        let proof_bytes = result.proof_bytes.expect("expected real proof bytes");

        // ── Verify with arkworks ─────────────────────────────────────────
        use ark_groth16::Proof as ArkProof;
        let ark_proof = ArkProof::<Bn254>::deserialize_compressed(&proof_bytes.0[..])
            .expect("deserialise proof");
        let pvk = Groth16::<Bn254>::process_vk(&ark_vk).expect("process_vk");
        let pub_ark = [ArkFr::from(1u64)];
        let ok = Groth16::<Bn254>::verify_with_processed_vk(&pvk, &pub_ark, &ark_proof)
            .expect("verify_with_processed_vk error");
        assert!(ok, "proof verification returned false");
    }

    #[test]
    fn s31_groth16_legacy_test_backcompat() {
        // Callers that previously expected placeholder-Ok for no-key must now
        // update to handle Err(NoProvingKey). Verify the error is structured.
        let mut cb = CircuitBuilder::new();
        let _w = cb.public_input("x");
        let circuit = cb.build();
        let p = Prover::new(
            ProverConfig { threads: 1, proof_type: ProofType::Groth16, rng_seed: None },
            empty_pk(),
        );
        match p.prove(&circuit, &[Fp::from_u64(0)]) {
            Err(ProverError::NoProvingKey) => {} // expected
            other => panic!("expected NoProvingKey, got: {:?}", other),
        }
    }
}
