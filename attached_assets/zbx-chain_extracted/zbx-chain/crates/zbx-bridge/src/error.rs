use thiserror::Error;

#[derive(Debug, Error)]
pub enum BridgeError {
    // ── Amount / token validation ─────────────────────────────────────────
    #[error("amount {0} below minimum bridge amount")]
    AmountTooSmall(u128),

    #[error("zero amount is not allowed")]
    ZeroAmount,

    #[error("amount exceeds max per-tx limit of {max}")]
    ExceedsMaxPerTx { max: u128 },

    #[error("daily bridge limit exceeded: limit={limit}, used={used}, requested={requested}")]
    DailyLimitExceeded { limit: u128, used: u128, requested: u128 },

    // ── Token whitelist ───────────────────────────────────────────────────
    #[error("token {0} is not whitelisted for bridging")]
    TokenNotWhitelisted(String),

    #[error("token {0} is currently disabled for bridging")]
    TokenDisabled(String),

    // ── Chain ─────────────────────────────────────────────────────────────
    #[error("unsupported target chain: {0}")]
    UnsupportedChain(u64),

    // ── Multisig / signatures ─────────────────────────────────────────────
    #[error("insufficient multisig confirmations: {got}/{required}")]
    InsufficientConfirmations { got: usize, required: usize },

    #[error("invalid signature: {0}")]
    InvalidSignature(String),

    // ── Request lifecycle ─────────────────────────────────────────────────
    #[error("duplicate bridge request: {0}")]
    DuplicateRequest(String),

    #[error("bridge request expired (max age: 24h)")]
    Expired,

    #[error("bridge request not found: {0}")]
    NotFound(String),

    // ── Proof ─────────────────────────────────────────────────────────────
    #[error("proof verification failed: {0}")]
    ProofInvalid(String),

    // ── Replay protection (H-03 fix / ZBX-H-03) ──────────────────────────
    /// Returned by `MultisigAuth::verify_and_consume()` when the operation
    /// hash has already been consumed by a prior execution.  The inner string
    /// is the hex-encoded `msg_hash` of the replayed operation.
    #[error("operation already executed — replay attempt rejected: {0}")]
    ReplayedOperation(String),

    // ── Persistence (OUT1 fix) ────────────────────────────────────────────
    /// Returned by `BridgeRelayer::execute()` when the durable write of the
    /// spent-operation hash to RocksDB fails.
    ///
    /// The execution is ABORTED — the bridge request remains in `pending` and
    /// the in-memory `spent_operations` set is NOT updated.  The caller should
    /// surface this as a node error, investigate the storage subsystem, and
    /// retry the execution.  Under no circumstances should the caller skip the
    /// persistence step and call `mark_spent` directly — that would silently
    /// recreate the replay-vulnerability this fix was introduced to close.
    #[error("bridge spent-op persistence failed: {0}")]
    PersistenceFailure(String),

    // ── Source-chain binding (OUT2 fix) ───────────────────────────────────
    /// Returned when a bridge request's `source_chain_id` does not match the
    /// chain ID on which this `BridgeRelayer` instance is running.
    ///
    /// This is a defence-in-depth guard: a request that was signed for chain A
    /// cannot be submitted to, confirmed on, or executed by a relayer instance
    /// running on chain B — even if both bridges share the same relayer key set.
    #[error("source chain mismatch: expected chain {expected}, got {got}")]
    SourceChainMismatch { expected: u64, got: u64 },

    // ── Emergency pause ───────────────────────────────────────────────────
    #[error("bridge is paused — no new requests accepted")]
    Paused,
}
