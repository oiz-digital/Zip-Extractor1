use thiserror::Error;

#[derive(Debug, Error)]
pub enum KeystoreError {
    /// Returned by `KeystoreWallet::from_keyfile` when the MAC check fails.
    /// This is the canonical "wrong password OR tampered keyfile" case —
    /// the two are intentionally indistinguishable so callers cannot use
    /// timing or error-message differences to brute-force passwords.
    #[error("decryption failed: invalid password or corrupted keyfile")]
    InvalidPassword,
    /// Legacy alias kept for back-compat with older callers.
    #[error("decryption failed: wrong password")]
    WrongPassword,
    #[error("keyfile not found: {0}")]
    NotFound(String),
    #[error("invalid keyfile format: {0}")]
    InvalidFormat(String),
    #[error("key already exists for address {0}")]
    AlreadyExists(String),
    #[error("unsupported KDF: {0}")]
    UnsupportedKdf(String),
    #[error("unsupported cipher: {0}")]
    UnsupportedCipher(String),
    #[error("MAC verification failed — file may be corrupted")]
    MacMismatch,
    /// A cryptographic primitive (scrypt, AES, ECDSA validation) failed.
    #[error("crypto error: {0}")]
    Crypto(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}