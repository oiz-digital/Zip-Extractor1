//! XCL error types.

use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq)]
pub enum XclError {
    // ── Channel errors ───────────────────────────────────────────────────
    #[error("channel {0} not found")]
    ChannelNotFound(String),

    #[error("channel {0} is not open (state: {1})")]
    ChannelNotOpen(String, String),

    #[error("channel {0} is already closed")]
    ChannelClosed(String),

    #[error("channel open handshake ordering mismatch")]
    OrderingMismatch,

    // ── Client errors ────────────────────────────────────────────────────
    #[error("foreign client {0} not found")]
    ClientNotFound(String),

    #[error("foreign header at height {0} not found")]
    HeaderNotFound(u64),

    #[error("foreign header BLS QC verification failed: {0}")]
    InvalidQc(String),

    #[error("foreign header height {0} is not newer than stored height {1}")]
    StaleHeader(u64, u64),

    #[error("foreign client has no validator set — cannot verify QC")]
    NoValidatorSet,

    // ── Proof errors ─────────────────────────────────────────────────────
    #[error("state proof verification failed: {0}")]
    ProofInvalid(String),

    #[error("commitment mismatch: expected {expected}, got {got}")]
    CommitmentMismatch { expected: String, got: String },

    #[error("packet receipt already exists for channel {0} seq {1}")]
    PacketAlreadyReceived(String, u64),

    #[error("packet acknowledgement already exists for channel {0} seq {1}")]
    AlreadyAcknowledged(String, u64),

    // ── Packet errors ────────────────────────────────────────────────────
    #[error("packet timeout: height {packet_height} ≥ timeout {timeout}")]
    PacketTimeout { packet_height: u64, timeout: u64 },

    #[error("packet has not yet timed out — cannot process timeout")]
    PacketNotTimedOut,

    #[error("sequence {got} out of order — expected {expected}")]
    SequenceOutOfOrder { expected: u64, got: u64 },

    #[error("no pending commitment for channel {0} seq {1}")]
    NoCommitment(String, u64),

    // ── Transfer errors ──────────────────────────────────────────────────
    #[error("insufficient escrow balance: need {need}, have {have}")]
    InsufficientEscrow { need: u128, have: u128 },

    #[error("amount overflow in transfer")]
    AmountOverflow,

    #[error("invalid denom '{0}'")]
    InvalidDenom(String),

    #[error("FT packet decode failed: {0}")]
    DecodeFailed(String),

    #[error("invalid packet data: {0}")]
    InvalidPacketData(String),

    #[error("unsupported application protocol: app_id=0x{0:02x}")]
    UnsupportedApp(u8),

    // ── Generic ──────────────────────────────────────────────────────────
    #[error("chain ID mismatch: expected {expected}, got {got}")]
    ChainIdMismatch { expected: u64, got: u64 },

    #[error("XCL internal error: {0}")]
    Internal(String),
}
