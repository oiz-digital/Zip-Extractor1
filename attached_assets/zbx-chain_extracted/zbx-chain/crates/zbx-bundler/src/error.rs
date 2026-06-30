//! Bundler error types.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum BundlerError {
    #[error("simulation failed: {0}")]
    SimulationFailed(String),

    #[error("UserOperation gas too high: {0}")]
    GasTooHigh(u64),

    #[error("pre-verification gas too low (min 21000)")]
    PreVerificationGasTooLow,

    #[error("unsupported entry point: {0}")]
    UnsupportedEntryPoint(String),

    #[error("invalid sender address")]
    InvalidSender,

    #[error("missing signature")]
    MissingSignature,

    #[error("calldata too large: {0} bytes")]
    CalldataTooLarge(usize),

    #[error("empty UserOperation (no initCode or callData)")]
    EmptyOperation,

    #[error("verification gas limit too low")]
    VerificationGasTooLow,

    #[error("call gas limit is zero but callData is non-empty")]
    CallGasZero,

    #[error("empty bundle")]
    EmptyBundle,

    #[error("relay error: {0}")]
    Relay(String),

    #[error("bundler rpc error: {0}")]
    Rpc(String),

    /// SEC-2026-05-09 Pass-15 (HIGH-R05): UserOp time window expired
    /// or not yet active. Bundler refuses to include in a bundle.
    #[error("UserOp expired: validAfter={valid_after} validUntil={valid_until} now={now}")]
    Expired { valid_after: u64, valid_until: u64, now: u64 },

    /// ZBX_BUNDLER_PRIVKEY env var is not set and no key was provided
    /// to BundleRelay::new(). Cannot sign bundle transactions.
    #[error("bundler private key not set (set ZBX_BUNDLER_PRIVKEY env var)")]
    MissingPrivKey,

    /// The bundler private key bytes are malformed.
    #[error("invalid bundler private key: {0}")]
    InvalidPrivKey(String),

    /// Bundle transaction submitted but not mined within the deadline.
    #[error("bundle inclusion timeout: tx {tx} not mined within 120 s")]
    InclusionTimeout { tx: String },

    /// The submitted bundle transaction was mined but reverted on-chain.
    #[error("bundle transaction reverted on-chain at block {block}")]
    BundleReverted { block: u64 },

    /// UserOperation simulation (simulateValidation) failed with a
    /// genuine validation error (not the expected intentional revert).
    #[error("simulateValidation rejected op: {0}")]
    SimulationRejected(String),
}