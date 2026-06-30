//! FROST Threshold Signatures for ZBX.
//!
//! # What is Threshold Signing?
//!
//! Instead of one validator holding a private key, the key is split into
//! n "shares" distributed among m validators. Any t-of-m (e.g. 2-of-3,
//! or 67-of-100) validators can collaborate to produce a valid signature,
//! but no subset of t-1 validators can forge one.
//!
//! # FROST (Flexible Round-Optimized Schnorr Threshold)
//!
//! FROST is a state-of-the-art 2-round threshold Schnorr signature scheme:
//! - Round 1: Each signer generates and broadcasts a nonce commitment
//! - Round 2: Each signer uses the aggregate commitment to produce a partial sig
//! - Combiner: Aggregates partial sigs into one Schnorr sig (32 bytes)
//!
//! The final signature is a standard Schnorr sig — verifiable by any normal
//! validator without knowing it was threshold-produced.
//!
//! # ZBX Use Cases
//! - Validator committee signing for block finality (replaces single BLS sig)
//! - Bridge multi-sig (replaces N-of-M on-chain multi-sig)
//! - Slashing proof aggregation
//! - Keystore recovery (social recovery of validator keys)
//!
//! # Threshold parameters
//! - t = ⌈2/3 × n⌉ + 1  (BFT threshold: 2/3 + 1 validators)
//! - n = validator committee size (typically 100–500)

pub mod aggregate;
pub mod bls_aggregate;
pub mod dkg;
pub mod error;
pub mod keyshare;
pub mod round1;
pub mod round2;
pub mod scalar;
pub mod verify;

pub use aggregate::ThresholdSig;
pub use bls_aggregate::{
    BLSError, BLSQuorumCertificate, BlsAggSignature, BlsProofOfPossession,
    BlsPubKey, BlsSignature, ValidatorBitmap,
    bls_aggregate, bls_aggregate_pubkeys, bls_batch_verify,
    bls_fast_agg_verify, bls_sign, bls_verify_single,
};
pub use error::ThresholdError;
pub use keyshare::{KeyShare, GroupKey};

/// Minimum threshold for BFT security: t = 2n/3 + 1.
pub fn bft_threshold(n: usize) -> usize {
    (n * 2 / 3) + 1
}