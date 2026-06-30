//! Error types for post-quantum cryptography operations.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PqError {
    #[error("invalid Dilithium public key: expected {expected} bytes, got {got}")]
    InvalidPublicKeyLength { expected: usize, got: usize },

    #[error("invalid Dilithium signature: expected {expected} bytes, got {got}")]
    InvalidSignatureLength { expected: usize, got: usize },

    #[error("invalid Kyber public key: expected {expected} bytes, got {got}")]
    InvalidKyberKeyLength { expected: usize, got: usize },

    #[error("Dilithium signature verification failed")]
    SignatureVerificationFailed,

    #[error("Kyber decapsulation failed: ciphertext invalid or key mismatch")]
    DecapsulationFailed,

    #[error("hybrid verification failed: both ECDSA and Dilithium signatures invalid")]
    HybridVerificationFailed,

    #[error("key derivation failed: {0}")]
    KeyDerivationFailed(String),

    #[error("random number generation failed")]
    RngFailed,

    #[error("serialization error: {0}")]
    Serialization(String),
}
