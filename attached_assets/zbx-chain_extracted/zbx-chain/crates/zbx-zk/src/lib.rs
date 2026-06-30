//! ZK proof system — circuits, prover (legacy/off-chain only), verifier.
//!
//! # S26 hardening (Groth16)
//!
//! [`verifier::Groth16Verifier`] is the **production** API: it wires
//! arkworks Groth16 over BN254 (the same curve as Ethereum's pairing
//! precompiles). Real proofs should be generated off-chain via snarkjs /
//! circom / arkworks tooling and verified through this struct.
//!
//! The legacy [`prover::Prover`] type is kept for shape compatibility but
//! emits all-zero placeholder bytes for Groth16; the legacy
//! [`verifier::Verifier`] facade now ALWAYS rejects so no caller is
//! silently fooled.
//!
//! # S31 hardening (PLONK)
//!
//! The new [`plonk`] module ships the operator-supplied trusted-setup
//! workflow ([`plonk::PlonkSrsBytes`], [`plonk::srs_hash`],
//! [`plonk::PLONK_SRS_SENTINEL_BYTES`]) and a fail-closed
//! [`plonk::PlonkVerifier`]. PLONK proving in [`prover::Prover::prove`]
//! now hard-errors with [`prover::ProverError::PlonkNotImplemented`]
//! (previously emitted all-zero placeholder bytes that LOOKED
//! well-formed). Real PLONK wiring is gated on upstream `ark-plonk`
//! BN254 stabilisation; see [`plonk`] module docs for the future
//! integration path.

pub mod circuit;
pub mod plonk;
pub mod prover;
pub mod stark;
pub mod verifier;

pub use circuit::{Circuit, CircuitBuilder, Fp};
pub use verifier::{
    // Legacy compat surface (always-reject after S26)
    Verifier, Proof, VerificationKey, ProofType, Groth16Proof, PlonkProof,
    // Real S26 verifier
    Groth16Verifier, Groth16ProofBytes, VerifyingKeyBytes, VerifyError,
    scalar_from_u64, scalar_from_bytes, serialize_public_inputs,
};
pub use prover::{Prover, ProverConfig, ProvingKey, ProverError};
// S31 — PLONK trusted-setup workflow + fail-closed verifier.
pub use plonk::{
    PlonkSrsBytes, PlonkVerifyingKeyBytes, PlonkVerifier,
    PLONK_SRS_SENTINEL_BYTES,
    srs_hash, is_srs_sentinel, is_srs_all_zero,
};
// ZEP-019 — STARK verifier (no trusted setup)
pub use stark::{
    StarkConfig, StarkField, StarkProof, StarkVerifier, StarkError,
    FriLayer, FriDecommitment, Fp as StarkFp, GOLDILOCKS_PRIME,
};
