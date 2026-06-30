//! zbx-types: Core primitive types for the Zebvix L1 blockchain.
//!
//! Provides Address (20-byte EVM-compatible), H256, U256, Block, Transaction,
//! Receipt, AccountState and all associated error types.

pub mod account;
pub mod activation;
pub mod address;
pub mod block;
pub mod consensus;
pub mod defi;
pub mod error;
pub mod events;
pub mod execution;
pub mod feature_flags;
pub mod finality;
// ── hardfork module REMOVED (Policy: live-chain-only upgrades) ──
//
// The `hardfork` module (a `HardFork` enum + `HardForkSchedule` keyed
// by block height, loadable from node TOML) has been deleted as part
// of the no-hard-fork governance lock-down. RATIONALE:
//
//   * Any operator-side or genesis-side hard-fork schedule is a
//     trust surface: a node operator (or a malicious genesis config)
//     could swap consensus rules without on-chain approval.
//   * The ONLY sanctioned upgrade pathway on this chain is the
//     `VersionRegistry` mutated through `ProposalRegistry` /
//     `UpgradeProposal` execution — i.e. an on-chain ZEP that
//     passed quorum + threshold voting and reached its scheduled
//     activation height inside the state-transition function.
//   * Per-EVM-era selectors (`london_block` / `shanghai_block` /
//     `cancun_block` in `zbx-genesis::ChainConfig`) remain ONLY as
//     genesis-immutable EVM-dialect pins — they CANNOT be mutated
//     post-launch by any party. They are not hard-forks; they are
//     genesis-time EVM-compatibility selectors.
//
// To add a new protocol feature post-launch:
//   1. File a ZEP (`UpgradeProposal`) with a `RegistryUpgrade` payload.
//   2. Reach quorum (1M ZBX) + approval threshold via on-chain voting.
//   3. `ProposalRegistry::ready_to_execute` surfaces it at the
//      activation height; the STF applies it via
//      `VersionRegistry::apply` — atomically updating
//      `ModuleVersions` + `ActivationSchedule` + `FeatureFlags` +
//      `StorageVersion` in the world state.
//
// See: `version_registry.rs`, `activation.rs`, `governance.rs`.
pub mod governance;
pub mod mempool;
pub mod module_version;
pub mod network;
pub mod oracle;
pub mod payid;
pub mod pinned_genesis;
pub mod proposer;
pub mod receipt;
pub mod slashing;
pub mod staking_tx;
pub mod storage_version;
pub mod transaction;
pub mod validation;
pub mod version_registry;
pub mod vm;

pub use account::AccountState;
pub use activation::{Activation, ActivationSchedule};
pub use address::Address;
pub use block::{Block, BlockBody, BlockHeader};
pub use consensus::{ConsensusParams, ValidatorInfo, ValidatorSet, ViewChange};
pub use error::ZbxError;
pub use execution::{
    BlockGasTracker, DeterministicClock, ExecutionContext, ExecutionLimits, GasError, GasMeter,
};
pub use feature_flags::{FeatureFlags, Flag};
pub use governance::{
    ProposalId, ProposalRegistry, ProposalStatus, UpgradeProposal, Vote, VoteTally,
};
pub use mempool::{
    MempoolPolicy, MempoolReject, PriorityRule, RateLimitRule,
};
pub use module_version::{ModuleVersion, ModuleVersions};
pub use oracle::{
    AggregationKind, FeedId, OracleFeedSpec, OraclePolicy, OracleReject,
    OracleSubmission,
};
pub use pinned_genesis::{
    is_all_zero as is_genesis_all_zero,
    is_sentinel as is_genesis_sentinel,
    pinned_for as pinned_genesis_for,
    verify_pinned, verify_pinned_with_policy,
    MAINNET_GENESIS_HASH, PinError, PinPolicy, SENTINEL_HASH, TESTNET_GENESIS_HASH,
};
pub use proposer::{
    select_proposer, ProposerAlgorithm, ProposerSchedule, ProposerSeed,
};
pub use receipt::{Log, TransactionReceipt};
pub use slashing::{
    SlashingEvidence, SlashingFault, SlashingParams, SlashingPenalty, SlashingRecord,
    SlashingRegistry,
};
pub use storage_version::{Migration, MigrationContext, MigrationPlan, StorageVersion};
pub use transaction::{SignedTransaction, Transaction};
pub use validation::{SignatureScheme, ValidationError, ValidationRules};
pub use version_registry::{RegistryUpgrade, VersionRegistry};
pub use vm::{
    HostCallId, OpcodeCost, OpcodeKind, SandboxLimits, VmError, VmPolicy,
};

// ---------------------------------------------------------------------------
// Fundamental type aliases — re-exported from `primitive-types` so the EVM
// can perform native 256-bit arithmetic. Both types are wire-stable: H256
// is a #[repr(C)] wrapper around `[u8; 32]`, and U256 is `[u64; 4]` with
// big-endian (de)serialisation helpers.
// ---------------------------------------------------------------------------

pub use primitive_types::{H160, H256, U256};

/// Chain ID for Zebvix mainnet (locked-in 2026-05-01).
pub const CHAIN_ID_MAINNET: u64 = 8989;

/// Chain ID for Zebvix public testnet AND devnet (locked-in 2026-05-01).
/// Devnet rides on the testnet preset; no separate chain ID is allocated.
pub const CHAIN_ID_TESTNET: u64 = 8990;

/// Backward-compatible alias for `CHAIN_ID_MAINNET`. New code should use the
/// explicit `CHAIN_ID_MAINNET` (or `CHAIN_ID_TESTNET` on test/dev builds).
pub const CHAIN_ID: u64 = CHAIN_ID_MAINNET;

/// BIP-44 / SLIP-0044 coin-type registration for ZBX. **NOT** a chain ID.
/// Reserved value 7878 — see https://github.com/satoshilabs/slips/blob/master/slip-0044.md
/// Used by HD-wallet derivation paths: `m/44'/7878'/account'/change/index`.
pub const BIP44_COIN_TYPE_ZBX: u32 = 7878;

/// Maximum gas per block
pub const BLOCK_GAS_LIMIT: u64 = 30_000_000;

/// Total supply cap: 150 million ZBX (18 decimals)
pub const TOTAL_SUPPLY: u128 = 150_000_000 * 10u128.pow(18);

/// Block reward at genesis epoch (2 ZBX)
pub const INITIAL_BLOCK_REWARD: u128 = 2 * 10u128.pow(18);

/// Halving interval in blocks (~4 years at 2-second blocks)
pub const HALVING_INTERVAL: u64 = 63_072_000;

/// Compute block reward for a given block height applying ZBX halving schedule.
pub fn block_reward_at(height: u64) -> u128 {
    let halvings = height / HALVING_INTERVAL;
    if halvings >= 64 {
        return 0;
    }
    INITIAL_BLOCK_REWARD >> halvings
}

pub fn zero_hash() -> H256 {
    H256::zero()
}

pub fn h256_from_hex(s: &str) -> Result<H256, ZbxError> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    let b = hex::decode(s).map_err(|_| ZbxError::InvalidHex(s.to_string()))?;
    if b.len() != 32 {
        return Err(ZbxError::InvalidLength { expected: 32, got: b.len() });
    }
    Ok(H256::from_slice(&b))
}

/// Construct an [`H256`] from a slice; panics if the slice is not 32 bytes.
/// Convenience wrapper used widely by storage and trie code.
pub fn h256_from_slice(b: &[u8]) -> H256 {
    H256::from_slice(b)
}