//! zbx-staking: Proof-of-Stake staking protocol for Zebvix.
//!
//! Key parameters:
//! - Minimum self-stake: 100 000 ZBX
//! - Maximum validators: 100 (top-100 by total delegated stake)
//! - Epoch length: 172 800 blocks (~4 days at 2-second blocks)
//! - Unbonding period: 21 days (907 200 blocks at 2 s/block)
//! - Base commission rate: configurable per validator (0–20%)
//! - Slashing: 5% for double-sign, 0.01%/day for liveness fault
//! - Instant jail: 20 consecutive missed blocks (~100s at 5s/block)

pub mod delegation;
pub mod error;
pub mod rewards;
pub mod slashing;
pub mod slashing_v2;
pub mod staking_escrow;
pub mod validator;
// SEC-2026-05-09 Pass-11 — end-to-end slashing execution. Closes the
// HARD mainnet blocker named in
// `docs/SUBSYSTEM-MATURITY-AUDIT-2026-05-09.md`: detector + registry
// were in-memory only; no persistence, no auto-submission, no stake
// burn. `persistence` provides RocksDB-backed storage of
// EquivocationEvidence + SlashEvidenceRecord; `pipeline` orchestrates
// ingest → submit → finalize → burn with restart safety.
pub mod persistence;
pub mod pipeline;
// On-chain staking transaction dispatcher.
pub mod tx_handler;
pub mod delta;
// Governance proposal lifecycle helpers (try_finalize tick, ProposalRegistry I/O).
pub mod governance;

pub use delegation::{
    DelegationRegistry, DelegationRecord, DelegationKey, DelegationUnbond,
};
pub use error::StakingError;
pub use rewards::RewardDistributor;
pub use slashing::{SlashingDetector, SlashEvent};
pub use staking_escrow::{EscrowRegistry, EscrowEntry, UnbondingEntry};
pub use slashing_v2::{
    SlashingRegistryV2, SlashEvidenceRecord, SlashEvidenceV2,
    EvidenceStatus, EvidenceType, DoubleSignProof, InvalidBlockProof, BlockViolation,
    SubmitOutcome, FinalizedSlash,
    base_slash_bps, correlated_slash_bps, slash_amount_wei,
    APPEAL_WINDOW_BLOCKS, WHISTLEBLOWER_REWARD_BPS, EVIDENCE_BOND_WEI,
};
pub use validator::{Validator, ValidatorSet, ValidatorStatus};
pub use persistence::{
    EvidenceStore, BondEntry, BondKind, evidence_to_double_sign, evidence_id,
};
pub use pipeline::{
    SlashingPipeline, AppliedSlash, AppliedOverturn,
    apply_slash_burn, apply_slash_burn_v2,
};
pub use tx_handler::{
    BalanceAccess, decode_staking_call, dispatch_staking_tx, dispatch_file_appeal_tx,
    is_staking_destination,
    STAKING_GAS_REGISTER, STAKING_GAS_DELEGATE, STAKING_GAS_UNDELEGATE,
    STAKING_GAS_WITHDRAW, STAKING_GAS_CLAIM, STAKING_GAS_CLAIM_DELEGATOR,
    STAKING_GAS_FILE_APPEAL, STAKING_GAS_PROPOSE_UPGRADE, STAKING_GAS_CAST_VOTE,
};
pub use governance::try_finalize_all_pending;
pub use delta::StakingDelta;

/// Minimum self-stake to register as a validator (100k ZBX in wei).
pub const MIN_SELF_STAKE: u128 = 100_000 * 10u128.pow(18);
/// Epoch length in blocks.
pub const EPOCH_LENGTH: u64 = 172_800;
/// Unbonding period in blocks. Single source of truth re-exported
/// from `zbx_types::staking_tx::UNBONDING_PERIOD_BLOCKS` so that
/// every consumer (pipeline, RPC, tests, integration suite) reads
/// the SAME canonical 21-day window. Pre-Round-5 this crate also
/// declared a stale 14-day legacy constant — removed to eliminate
/// dual-source-of-truth drift.
pub use zbx_types::staking_tx::UNBONDING_PERIOD_BLOCKS as UNBONDING_PERIOD;
/// Maximum active validators.
pub const MAX_VALIDATORS: usize = 100;
/// Double-sign slash fraction (5%).
pub const SLASH_DOUBLE_SIGN: u128 = 500; // bps
/// Liveness fault slash per day (0.01%).
pub const SLASH_LIVENESS_DAILY: u128 = 1; // bps
/// Consecutive missed blocks before a validator is INSTANTLY jailed.
/// At 5 s/block: 20 blocks ≈ 100 seconds of silence → node is considered down.
/// This fires per-block (not at epoch end) so the active set is updated immediately.
pub const MAX_CONSECUTIVE_MISSED: u64 = 20;
/// Reward distribution interval in blocks.
///
/// Staking rewards (base subsidy + accumulated transaction fees) are credited to
/// validators and delegators once every `REWARD_INTERVAL` blocks rather than
/// every block.  The executor must accumulate transaction fees across the window
/// and pass the total to `distribute_block_reward` at the boundary block.
///
/// At 2 s/block: 100 blocks ≈ 200 seconds (~3.3 min) between distributions.
pub const REWARD_INTERVAL: u64 = 100;