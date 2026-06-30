use thiserror::Error;

#[derive(Debug, Error)]
pub enum MevError {
    #[error("bundle simulation failed: {0}")]
    SimulationFailed(String),
    #[error("bundle tx reverted at index {index}: {reason}")]
    BundleTxReverted { index: usize, reason: String },
    #[error("bid too low: minimum {min}, got {bid}")]
    BidTooLow { min: u128, bid: u128 },
    #[error("slot auction expired at block {0}")]
    SlotExpired(u64),
    #[error("private tx decryption failed: {0}")]
    DecryptionFailed(String),
    #[error("bundle gas exceeds block limit: {used}/{limit}")]
    BundleGasExceeded { used: u64, limit: u64 },
    #[error("commit-reveal: reveal does not match commit hash")]
    RevealMismatch,
    #[error("commit-reveal: reveal too early (committed at block {committed_at}, earliest reveal block {earliest_reveal})")]
    RevealTooEarly { committed_at: u64, earliest_reveal: u64 },
    #[error("duplicate bundle id: {0}")]
    DuplicateBundle(String),
    /// S29 — wire-supplied EncryptedTx failed integrity checks before
    /// being inserted into the private mempool (id mismatch, replay, etc.).
    #[error("invalid encrypted tx: {0}")]
    InvalidEncryptedTx(String),
}