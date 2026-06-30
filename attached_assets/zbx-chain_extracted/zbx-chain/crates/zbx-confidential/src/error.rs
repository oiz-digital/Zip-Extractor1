//! Error types for confidential transaction operations.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfidentialError {
    #[error("range proof verification failed: amount may be negative or overflow u64")]
    RangeProofInvalid,

    #[error("balance conservation violated: inputs - outputs != fee")]
    BalanceConservationFailed,

    #[error("commitment opening invalid: value/blinding do not match commitment")]
    CommitmentOpeningFailed,

    #[error("stealth address derivation failed: {0}")]
    StealthDerivationFailed(String),

    #[error("note decryption failed: wrong view key or corrupted ciphertext")]
    NoteDecryptionFailed,

    #[error("invalid commitment length: expected 32 bytes, got {0}")]
    InvalidCommitmentLength(usize),

    #[error("serialization error: {0}")]
    Serialization(String),

    #[error("insufficient inputs: {inputs} inputs cover {input_sum}, need {required}")]
    InsufficientInputs {
        inputs: usize,
        input_sum: u128,
        required: u128,
    },
}
