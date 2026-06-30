//! zbx-prover — ZK proof system for Zebvix Chain v0.2.
//!
//! # Proof types
//!
//! | Type | Use case | Proof size | Verify time |
//! |------|----------|------------|-------------|
//! | StateProof | Light client account lookup | ~1 KB | <1 ms |
//! | TxProof | Validate tx without replay | ~48 KB | ~5 ms |
//! | BlockProof | Full block execution proof | ~320 KB | ~50 ms |
//! | FraudProof | Bridge dispute / rollback | ~128 KB | ~20 ms |
//! | RecursiveProof | Aggregate N blocks | ~48 KB | ~5 ms |
//!
//! # Quick start
//!
//! ```rust
//! use zbx_prover::{Prover, StateProof};
//!
//! // Prove account balance at block 10_000.
//! let proof = Prover::new().prove_state(&witness)?;
//! assert!(proof.verify(&public_inputs)?);
//! ```
//!
//! # Architecture
//!
//! ```
//! Execution trace
//!      │
//!      ▼
//! WitnessGenerator   ← raw block data (txs, state diffs)
//!      │  witness (private inputs)
//!      ▼
//! Circuit            ← algebraic constraints (AIR)
//!      │  constraint system
//!      ▼
//! Prover             ← FRI-STARK proof generation
//!      │  proof π
//!      ▼
//! Verifier           ← verify π against public inputs
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

pub mod circuit;
pub mod error;
pub mod field;
pub mod fraud_proof;
pub mod params;
pub mod prover;
pub mod recursive;
pub mod state_proof;
pub mod transcript;
pub mod verifier;
pub mod witness;

pub use error::ProverError;
pub use prover::Prover;
pub use verifier::Verifier;
pub use state_proof::{StateProof, StateProofRequest};
pub use fraud_proof::{FraudProof, FraudProofType};
pub use circuit::{Circuit, CircuitType};
pub use witness::{Witness, BlockWitness, TxWitness};
pub use recursive::RecursiveProof;

/// ZBX Chain STARK security level (bits of security).
/// 128 bits: requires attacker to do 2^128 work to break proof.
pub const SECURITY_BITS: usize = 128;

/// FRI protocol rate (1/blowup_factor). Lower = more secure, larger proof.
pub const FRI_BLOWUP_FACTOR: usize = 4;

/// Number of FRI query rounds for 128-bit security.
pub const FRI_NUM_QUERIES: usize = 40;

/// Maximum block size supported by the prover (in transactions).
pub const MAX_BLOCK_TXS: usize = 10_000;

/// Version identifier embedded in all proofs.
pub const PROOF_VERSION: u8 = 2;