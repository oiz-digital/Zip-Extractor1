//! ZK proof verifier for ZBX — **real** Groth16 over BN254.
//!
//! # S26 hardening pass
//!
//! Prior to S26, `Verifier::verify_groth16` carried the comment
//! `// Simplified: always passes for demo` and unconditionally returned
//! `Ok(true)`. That meant any byte blob would have been accepted by the
//! verifier — a CRIT security failure.
//!
//! S26 introduces [`Groth16Verifier`], wired into the `arkworks` Groth16
//! implementation over BN254. The same curve is used by Ethereum's
//! `ecAdd` / `ecMul` / `ecPairing` precompiles (EIP-196 / EIP-197), so
//! proofs verifiable here can be re-verified by
//! `contracts/ZbxGroth16Verifier.sol` on-chain.
//!
//! The legacy [`Verifier`] / [`Proof`] enum API is preserved for backward
//! compatibility with [`crate::prover`] (which only emits all-zero
//! placeholder bytes — see its module docs). It now ALWAYS rejects, so no
//! caller can be tricked by the old "demo" stub.
//!
//! # Public-input field
//!
//! Public inputs are passed as little-endian 32-byte chunks (canonical
//! BN254 Fr scalar serialisation, identical to snarkjs / circom output).
//! Use [`scalar_from_bytes`] to convert your own integer values.

use ark_bn254::{Bn254, Fr};
use ark_ff::PrimeField;
use ark_groth16::{Groth16, Proof as ArkProof, VerifyingKey as ArkVk, PreparedVerifyingKey};
use ark_serialize::{CanonicalDeserialize, CanonicalSerialize, Compress, Validate};
use ark_snark::SNARK;

// ──────────────────────────────────────────────────────────────────────────
// Errors
// ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    #[error("invalid proof bytes: {0}")]
    InvalidProofBytes(String),
    #[error("invalid verification key bytes: {0}")]
    InvalidVkBytes(String),
    #[error("invalid public input bytes")]
    InvalidPublicInput,
    #[error("pairing check failed")]
    PairingFailed,
    #[error("input count mismatch: expected {expected}, got {got}")]
    InputCountMismatch { expected: usize, got: usize },
    #[error("legacy placeholder proof refused — use Groth16Verifier with real arkworks proof bytes")]
    LegacyPlaceholderRefused,
    /// S31 — PLONK proving system is not wired in this build. Caller
    /// reached the (future) real PLONK verifier path (see
    /// [`crate::plonk::PlonkVerifier::verify`]) which is gated on
    /// upstream `ark-plonk` BN254 stabilisation. Distinct from
    /// [`Self::LegacyPlaceholderRefused`] so callers can programmatically
    /// distinguish "PLONK not implemented" from "you called the deprecated
    /// always-rejecting facade".
    #[error("PLONK verifier not implemented in this build — see crates/zbx-zk/src/plonk.rs module docs for the future wiring path")]
    PlonkNotImplemented,
    /// S31 — operator has not supplied a real PLONK universal SRS
    /// (Powers-of-Tau / KZG ceremony output). The configured SRS was
    /// either the sentinel marker
    /// ([`crate::plonk::PLONK_SRS_SENTINEL_BYTES`]) or all-zero /
    /// empty. The chain refuses to construct a PLONK verifier in this
    /// state to prevent any future call from accidentally trusting
    /// uninitialised setup material.
    #[error("PLONK SRS not initialised — operator must supply real Powers-of-Tau ceremony output")]
    PlonkSrsNotInitialized,
}

// ──────────────────────────────────────────────────────────────────────────
// New (S26) — real Groth16Verifier
// ──────────────────────────────────────────────────────────────────────────

/// A serialized Groth16 verification key (arkworks canonical compressed form).
///
/// Use [`Groth16Verifier::new`] to deserialize once, then re-use the
/// resulting verifier for every check.
#[derive(Debug, Clone)]
pub struct VerifyingKeyBytes(pub Vec<u8>);

/// A serialized Groth16 proof (arkworks canonical compressed form).
#[derive(Debug, Clone)]
pub struct Groth16ProofBytes(pub Vec<u8>);

/// A real Groth16 verifier holding a deserialized + prepared verifying key.
pub struct Groth16Verifier {
    pvk: PreparedVerifyingKey<Bn254>,
}

impl Groth16Verifier {
    /// Construct from compressed VK bytes. Validates the VK shape eagerly.
    pub fn new(vk_bytes: &[u8]) -> Result<Self, VerifyError> {
        let vk = ArkVk::<Bn254>::deserialize_with_mode(
            vk_bytes,
            Compress::Yes,
            Validate::Yes,
        )
        .map_err(|e| VerifyError::InvalidVkBytes(e.to_string()))?;
        let pvk = Groth16::<Bn254>::process_vk(&vk)
            .map_err(|_| VerifyError::PairingFailed)?;
        Ok(Self { pvk })
    }

    /// Verify a Groth16 proof against the configured VK and supplied public
    /// inputs. Returns `Ok(true)` only on a successful pairing check.
    pub fn verify(
        &self,
        proof_bytes: &[u8],
        public_inputs: &[Fr],
    ) -> Result<bool, VerifyError> {
        let proof = ArkProof::<Bn254>::deserialize_with_mode(
            proof_bytes,
            Compress::Yes,
            Validate::Yes,
        )
        .map_err(|e| VerifyError::InvalidProofBytes(e.to_string()))?;

        Groth16::<Bn254>::verify_with_processed_vk(&self.pvk, public_inputs, &proof)
            .map_err(|_| VerifyError::PairingFailed)
    }

    /// Convenience: parse public inputs from 32-byte little-endian chunks.
    pub fn parse_public_inputs(bytes: &[u8]) -> Result<Vec<Fr>, VerifyError> {
        if bytes.len() % 32 != 0 {
            return Err(VerifyError::InvalidPublicInput);
        }
        let mut out = Vec::with_capacity(bytes.len() / 32);
        for chunk in bytes.chunks(32) {
            out.push(Fr::from_le_bytes_mod_order(chunk));
        }
        Ok(out)
    }
}

/// Helper: convert a u64 to an Fr scalar.
pub fn scalar_from_u64(n: u64) -> Fr { Fr::from(n) }

/// Helper: convert raw little-endian bytes to an Fr scalar (mod prime).
pub fn scalar_from_bytes(bytes: &[u8]) -> Fr { Fr::from_le_bytes_mod_order(bytes) }

/// Serialize a slice of Fr scalars to canonical 32-byte LE chunks.
pub fn serialize_public_inputs(inputs: &[Fr]) -> Vec<u8> {
    let mut out = Vec::with_capacity(inputs.len() * 32);
    for s in inputs {
        let mut buf = Vec::with_capacity(32);
        s.serialize_compressed(&mut buf).expect("Fr serialize never fails");
        out.extend_from_slice(&buf);
    }
    out
}

// ──────────────────────────────────────────────────────────────────────────
// Legacy API surface — preserved for [`crate::prover`] backward compat.
//
// These types use fixed-size byte arrays that mirror the BN254 wire layout
// (G1 = 96 bytes uncompressed, G2 = 192 bytes uncompressed). They were the
// pre-S26 plumbing for the "demo" Verifier::verify_groth16 stub which
// returned Ok(true) unconditionally. The new behaviour is to ALWAYS reject:
// callers must migrate to [`Groth16Verifier`] which performs real pairings
// over arkworks-encoded proofs.
// ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProofType { Groth16, Plonk }

/// Legacy Groth16 proof byte layout. **Not** used by the real
/// [`Groth16Verifier`] — arkworks proofs use a compact compressed
/// serialisation instead. Kept only for backward compat with
/// [`crate::prover::Prover::prove`].
#[derive(Debug, Clone)]
pub struct Groth16Proof {
    pub a: [u8; 96],
    pub b: [u8; 192],
    pub c: [u8; 96],
}

/// Legacy PLONK proof byte layout. PLONK verification was never wired
/// in this codebase — kept only for backward compat with
/// [`crate::prover::Prover::prove`] (which now hard-errors on PLONK
/// after S31). For real PLONK in production use [`crate::plonk::PlonkVerifier`]
/// — currently a fail-closed stub awaiting upstream `ark-plonk` BN254
/// stabilisation. See the [`crate::plonk`] module docs for the
/// operator workflow and future wiring path.
#[derive(Debug, Clone)]
pub struct PlonkProof {
    pub commitments:    Vec<[u8; 96]>,
    pub evaluations:    Vec<crate::circuit::Fp>,
    pub opening_proof:  [u8; 96],
}

#[derive(Debug, Clone)]
pub enum Proof {
    Groth16(Groth16Proof),
    Plonk(PlonkProof),
}

#[derive(Debug, Clone)]
pub struct VerificationKey {
    pub vk_bytes: Vec<u8>,
}

/// Legacy verifier facade. Pre-S26 this returned `Ok(true)` for any input.
/// S26 hardens it to ALWAYS reject — callers must migrate to
/// [`Groth16Verifier`].
pub struct Verifier {
    proof_type: ProofType,
}

impl Verifier {
    pub fn new(_vk: VerificationKey, proof_type: ProofType) -> Result<Self, VerifyError> {
        Ok(Self { proof_type })
    }

    pub fn verify(&self, _proof: &Proof) -> Result<bool, VerifyError> {
        // SECURITY: this legacy API used to return Ok(true) for every input.
        // We now reject every call so no caller can be tricked. Real proofs
        // must go through [`Groth16Verifier`] above.
        Err(VerifyError::LegacyPlaceholderRefused)
    }

    pub fn proof_type(&self) -> ProofType { self.proof_type }
}

// ──────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_vk_bytes_rejected() {
        let bad = vec![0xff; 100];
        assert!(Groth16Verifier::new(&bad).is_err());
    }

    #[test]
    fn empty_vk_bytes_rejected() {
        assert!(Groth16Verifier::new(&[]).is_err());
    }

    #[test]
    fn parse_public_inputs_chunk_alignment() {
        assert!(Groth16Verifier::parse_public_inputs(&[0u8; 31]).is_err());
        assert!(Groth16Verifier::parse_public_inputs(&[0u8; 32]).is_ok());
        assert!(Groth16Verifier::parse_public_inputs(&[0u8; 64]).is_ok());
        assert!(Groth16Verifier::parse_public_inputs(&[0u8; 65]).is_err());
    }

    #[test]
    fn legacy_verifier_always_rejects_now() {
        // Pre-S26 this returned Ok(true). Post-S26 it MUST reject so no
        // caller is silently fooled.
        let vk = VerificationKey { vk_bytes: vec![] };
        let v = Verifier::new(vk, ProofType::Groth16).unwrap();
        let p = Proof::Groth16(Groth16Proof { a: [0u8; 96], b: [0u8; 192], c: [0u8; 96] });
        let r = v.verify(&p);
        assert!(matches!(r, Err(VerifyError::LegacyPlaceholderRefused)));
    }

    #[test]
    fn scalar_helpers_are_deterministic() {
        let a = scalar_from_u64(42);
        let b = scalar_from_u64(42);
        assert_eq!(a, b);
        assert_eq!(scalar_from_bytes(&[42u8]), scalar_from_u64(42));
    }
}
