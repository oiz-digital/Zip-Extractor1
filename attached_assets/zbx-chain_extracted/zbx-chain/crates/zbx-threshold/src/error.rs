use thiserror::Error;

#[derive(Debug, Error)]
pub enum ThresholdError {
    #[error("insufficient signers: required {required}, got {got}")]
    InsufficientSigners { required: usize, got: usize },
    #[error("empty signer set")]
    EmptySignerSet,
    #[error("invalid key share: index {0}")]
    InvalidKeyShare(u32),
    #[error("invalid nonce commitment from signer {0}")]
    InvalidNonce(u32),
    #[error("partial signature verification failed for signer {0}")]
    PartialSigInvalid(u32),
    #[error("DKG ceremony failed: {0}")]
    DkgFailed(String),
    #[error("threshold {threshold} exceeds participant count {total}")]
    ThresholdTooHigh { threshold: usize, total: usize },
    /// A scalar, point, or share could not be parsed / decoded / validated.
    #[error("invalid share or scalar: {0}")]
    InvalidShare(String),
}