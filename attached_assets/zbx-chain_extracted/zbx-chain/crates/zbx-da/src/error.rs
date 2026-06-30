//! DA layer error types.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum DaError {
    #[error("blob too large: {0} bytes (max 131072)")]
    BlobTooLarge(usize),

    #[error("too many blobs in tx: {0} (max 8)")]
    TooManyBlobs(usize),

    #[error("blob sidecar count mismatch: expected {expected}, got {got}")]
    SidecarCountMismatch { expected: usize, got: usize },

    #[error("blob versioned hash does not match sidecar commitment")]
    HashMismatch,

    #[error("KZG proof verification failed")]
    InvalidKzgProof,

    #[error("data unavailable at block {block}: only {available}/{expected} samples returned")]
    DataUnavailable { block: u64, available: usize, expected: usize },

    #[error("blob store error: {0}")]
    Store(String),

    #[error("DA layer not initialized")]
    NotInitialized,

    /// SEC-2026-05-09 Pass-12: DA sampling has no real peer protocol +
    /// KZG verification yet — fail-closed instead of returning fake
    /// "available" results.
    #[error("DA sampling not implemented — fail-closed (SEC-2026-05-09 Pass-12)")]
    NotImplemented,
}