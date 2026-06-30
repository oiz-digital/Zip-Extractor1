use thiserror::Error;

#[derive(Debug, Error)]
pub enum VerkleError {
    #[error("key not found in verkle trie")]
    KeyNotFound,
    #[error("maximum tree depth exceeded")]
    MaxDepthExceeded,
    #[error("invalid commitment length")]
    InvalidCommitment,
    #[error("proof verification failed")]
    ProofVerificationFailed,
    #[error("field element out of range")]
    FieldOutOfRange,
    #[error("IPA error: {0}")]
    Ipa(String),
}