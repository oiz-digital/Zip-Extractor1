use thiserror::Error;
use zbx_types::address::Address;

#[derive(Debug, Error)]
pub enum StakingError {
    #[error("validator {0:?} already registered")]
    AlreadyRegistered(Address),

    #[error("validator {0:?} not found")]
    NotFound(Address),

    #[error("insufficient self-stake: have {have} ZBX, need {need} ZBX")]
    InsufficientSelfStake { have: u128, need: u128 },

    #[error("unbonding period not elapsed: {remaining} blocks remaining")]
    UnbondingPeriod { remaining: u64 },

    #[error("validator is jailed")]
    Jailed,

    #[error("delegation too small: minimum is {0} wei")]
    DelegationTooSmall(u128),

    #[error("no pending rewards for {0:?}")]
    NoPendingRewards(Address),

    #[error("epoch not yet complete")]
    EpochNotComplete,

    // ── Slashing v2 variants (ZEP-023) ────────────────────────────────────────

    #[error("invalid slash evidence: {0}")]
    InvalidEvidence(String),

    #[error("duplicate evidence: this evidence has already been submitted")]
    DuplicateEvidence,

    #[error("evidence record not found")]
    EvidenceNotFound,

    #[error("appeal not allowed: evidence is not in Pending status")]
    AppealNotAllowed,

    #[error("appeal window has expired")]
    AppealWindowExpired,

    /// SEC-2026-05-09 Pass-11 — slashing-pipeline persistence layer.
    /// Wraps `zbx_storage::StorageError` strings + bincode encode /
    /// decode failures. Surfaces at `EvidenceStore` and `SlashingPipeline`
    /// boundaries so the caller can distinguish a logic error
    /// (`InvalidEvidence`) from a transient I/O / corruption issue.
    #[error("slashing persistence error: {0}")]
    Persistence(String),

    // ── Escrow helpers ────────────────────────────────────────────────────────

    #[error("invalid amount: amount must be non-zero")]
    InvalidAmount,

    #[error("insufficient stake")]
    InsufficientStake,

    #[error("unknown validator")]
    UnknownValidator,

    #[error("no delegation found")]
    NoDelegation,

    // ── Staking-tx pipeline ──────────────────────────────────────────────────

    /// The carrying transaction's `value` did not match the staking call's
    /// expected value (e.g. Withdraw / ClaimRewards must be value-zero).
    #[error("unexpected tx value: {got} wei (expected {expected} wei)")]
    UnexpectedValue { got: u128, expected: u128 },

    /// Caller tried to undelegate more than they had delegated.
    #[error("insufficient delegation: have {have} wei, requested {requested} wei")]
    InsufficientDelegation { have: u128, requested: u128 },

    /// Withdraw was called but no matured unbonding entries exist.
    #[error("nothing to withdraw: no matured unbonding entries")]
    NothingToWithdraw,

    /// `ClaimRewards` called by an address that is not a registered validator
    /// (delegator-side reward claims are deferred to a later sprint).
    #[error("claim rewards: {0:?} is not a registered validator")]
    NotAValidator(Address),

    /// Decoding the `StakingTx` payload from `tx.data` failed.
    #[error("malformed StakingTx payload: {0}")]
    BadPayload(String),

    /// On-chain staking-precompile escrow does not have enough wei to
    /// satisfy a withdraw — should never happen in steady state but is
    /// surfaced to avoid silent under-credit.
    #[error("staking escrow underflow: have {have} wei, need {need} wei")]
    EscrowUnderflow { have: u128, need: u128 },

    // ── Slashing-upgrade variants ────────────────────────────────────────────

    /// Caller tried to file a `FileAppeal` against a slash record whose
    /// offender address does not match the transaction sender. Only the
    /// validator under slash may appeal their own record.
    #[error("appeal must be filed by the offender (sender ≠ offender)")]
    AppealNotByOffender,

    /// Sender did not include the required appeal bond wei in the
    /// carrying transaction's value.
    #[error("appeal bond mismatch: got {got} wei, need {need} wei")]
    AppealBondMismatch { got: u128, need: u128 },

    /// `overturn_and_refund` was called on a record that is not in
    /// `Appealed` status. Overturn is only valid after a successful
    /// governance vote on an appealed record.
    #[error("overturn not allowed: record is not in Appealed status")]
    OverturnNotAllowed,
}