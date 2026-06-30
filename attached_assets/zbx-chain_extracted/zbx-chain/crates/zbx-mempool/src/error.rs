use thiserror::Error;
use zbx_types::address::Address;

#[derive(Debug, Error)]
pub enum MempoolError {
    #[error("transaction {0} already exists in pool")]
    AlreadyKnown(String),

    #[error("nonce too low for {addr}: expected {expected}, got {got}")]
    NonceTooLow { addr: Address, expected: u64, got: u64 },

    #[error("insufficient balance: has {balance} wei, needs {cost} wei")]
    InsufficientBalance { balance: u128, cost: u128 },

    #[error("gas limit {gas_limit} exceeds block gas limit {block_limit}")]
    GasLimitTooHigh { gas_limit: u64, block_limit: u64 },

    #[error("fee too low: effective tip {tip} < minimum {min}")]
    FeeTooLow { tip: u64, min: u64 },

    #[error("pending pool full ({0} slots occupied)")]
    PendingFull(usize),

    #[error("queued pool full ({0} slots occupied)")]
    QueuedFull(usize),

    #[error("invalid signature: {0}")]
    InvalidSignature(String),

    /// SEC-2026-05-09 (R2): per-sender slot cap exceeded.
    #[error("sender {addr} has too many pending+queued slots ({slots} > {max})")]
    TooManySlotsPerSender { addr: Address, slots: usize, max: usize },

    /// SEC-2026-05-09 (R2): cumulative wei reservation exceeds balance.
    /// Prevents an attacker from blowing past their on-chain balance via
    /// many low-value pending+queued txs that individually pass the
    /// per-tx balance check but together exceed it.
    #[error(
        "sender {addr} cumulative reservation {reserved} wei exceeds balance {balance} wei"
    )]
    CumulativeBalanceExceeded { addr: Address, reserved: u128, balance: u128 },

    /// SEC-2026-05-09 Pass-12 (mempool C1): replacement tx must bump fee
    /// by ≥ 12.5% (geth/erigon parity) — otherwise a free-replacement
    /// griefer can churn pool slots indefinitely.
    #[error("replacement underpriced for {addr} nonce {nonce}: new tip {new_tip} < required {required}")]
    ReplacementUnderpriced { addr: Address, nonce: u64, new_tip: u64, required: u64 },

    /// SEC-2026-05-09 Pass-12 (mempool H1): tx gas_limit must cover the
    /// EVM intrinsic cost (21000 base + 53000 for create) before admission.
    #[error("intrinsic gas too low for {addr}: gas_limit {gas_limit} < intrinsic {intrinsic}")]
    IntrinsicGasTooLow { addr: Address, gas_limit: u64, intrinsic: u64 },

    /// SEC-2026-05-09 Pass-13 (mempool T1-NONCE-GAP): tx nonce is too
    /// far ahead of sender's on-chain nonce. Without this cap a single
    /// sender can flood the queued pool with arbitrarily-far-future
    /// nonces (e.g. `nonce = 2^63`) that will never be promoted but
    /// occupy memory + slot budget until eviction.
    #[error("nonce gap too large for {addr}: tx nonce {nonce}, on-chain {on_chain}, max gap {max_gap}")]
    NonceGapTooLarge { addr: Address, nonce: u64, on_chain: u64, max_gap: u64 },
}