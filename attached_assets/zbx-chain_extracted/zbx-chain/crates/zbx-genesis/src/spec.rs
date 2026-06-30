//! Genesis specification — parsed from mainnet.toml or genesis.json.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Full genesis specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenesisSpec {
    /// Human-readable chain name.
    pub name: String,
    /// Chain ID (ZBX Mainnet = 8989; testnet+devnet share 8990).
    pub chain_id: u64,
    /// Genesis timestamp (Unix seconds).
    pub timestamp: u64,
    /// Initial gas limit.
    pub gas_limit: u64,
    /// Initial base fee (wei). 0 pre-EIP-1559.
    pub base_fee_per_gas: u64,
    /// Difficulty (0 for PoA / PoS chains).
    pub difficulty: u64,
    /// Extra data embedded in genesis block (max 32 bytes).
    pub extra_data: String,
    /// Chain configuration (fork activation blocks).
    pub config: ChainConfig,
    /// Initial account allocations: address → Allocation.
    pub alloc: HashMap<String, Allocation>,
    /// Genesis validators (PoA / BFT).
    pub validators: Vec<String>,
    /// Token pre-mints applied before block #1 (bypasses runtime minting limits).
    ///
    /// Mainnet default: ZUSD 100M → Foundation Treasury.
    /// Set by `zbx_contracts::genesis_mint::default_premints()`.
    #[serde(default)]
    pub token_premints: Vec<TokenPremint>,
}

/// EVM-dialect pin selectors, frozen at genesis.
///
/// ⚠️ NO-HARD-FORK POLICY (read this before touching ChainConfig):
///
/// These fields are NOT a hard-fork schedule. They are immutable
/// EVM-compatibility selectors baked into the genesis block — they
/// pin which EIPs are recognised on this chain from block 0. Once
/// the network is launched they cannot be changed by anyone:
///   * No operator CLI mutates them.
///   * No node TOML override re-loads them post-launch.
///   * No admin / governance proposal can re-target them — the
///     genesis block hash commits to this struct.
///
/// New post-launch protocol features MUST flow through the on-chain
/// `VersionRegistry` (via `UpgradeProposal` → ZEP vote →
/// `ProposalRegistry::ready_to_execute` → STF applies it). There is
/// no other sanctioned upgrade pathway on this chain.
///
/// Adding a new field here (e.g. a future EIP block selector) is a
/// GENESIS-ONLY change: it affects future chain launches, not the
/// running mainnet/testnet. Do not introduce mutator methods.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainConfig {
    pub chain_id:               u64,
    /// EIP-1559 activation block (0 = enabled from genesis).
    pub london_block:           u64,
    /// Merge activation block (PoS from genesis for ZBX).
    pub merge_netsplit_block:   Option<u64>,
    /// Shanghai (EIP-3855 PUSH0 etc.) activation block.
    pub shanghai_block:         Option<u64>,
    /// Cancun (EIP-4844 blob txs) activation block.
    pub cancun_block:           Option<u64>,
}

/// Initial account state in genesis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Allocation {
    /// Initial ZBX balance in wei (as hex string, e.g. "0x152d02c7e14af6800000").
    pub balance: String,
    /// Optional: pre-deployed contract bytecode (hex).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code:    Option<String>,
    /// Optional: contract storage slots.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage: Option<HashMap<String, String>>,
    /// Optional: account nonce (for pre-deployed contracts).
    #[serde(default)]
    pub nonce:   u64,
}

/// A genesis token pre-mint entry (for ZUSD initial supply).
///
/// Applied to the initial state trie **before block #1** — bypasses
/// runtime minting limits. The execution engine applies each entry via
/// `zbx_contracts::apply_premint()`.
///
/// Default mainnet entries (set in `GenesisSpec.token_premints`):
///   - ZUSD: 100,000,000 to Foundation Treasury
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenPremint {
    /// Token contract address (hex, e.g. `"0x...231D0001"` for ZUSD).
    pub contract:  String,
    /// Recipient address (hex). Mainnet = Foundation Treasury.
    pub recipient: String,
    /// Amount in token base units as hex string.
    pub amount:    String,
    /// Human-readable label (for logs and explorer).
    pub label:     String,
}

/// Errors raised by `GenesisSpec::validate`.
#[derive(Debug, thiserror::Error)]
pub enum GenesisError {
    #[error("genesis chain_id mismatch: spec.chain_id = {spec}, config.chain_id = {config}")]
    ChainIdMismatch { spec: u64, config: u64 },
    #[error("extra_data exceeds 32 bytes (got {0})")]
    ExtraDataTooLarge(usize),
    #[error("validators list is empty — chain cannot start")]
    NoValidators,
    #[error("alloc balance for {addr} is not parseable hex: {value}")]
    BadAllocBalance { addr: String, value: String },
    #[error("total allocated supply {0} exceeds 2^120 wei (sanity bound)")]
    AllocOverflow(u128),
}

impl GenesisSpec {
    /// Total allocated ZBX in genesis (sum of all balances).
    pub fn total_allocated(&self) -> u128 {
        self.alloc.values()
            .filter_map(|a| {
                let s = a.balance.trim_start_matches("0x");
                u128::from_str_radix(s, 16).ok()
            })
            .sum()
    }

    pub fn validator_count(&self) -> usize {
        self.validators.len()
    }

    /// Cross-validate the genesis spec against itself before any block is
    /// produced. The previous code would happily start a node where
    /// `spec.chain_id` and `spec.config.chain_id` disagreed, which
    /// eventually surfaces as silent EIP-155 signature-recovery failures
    /// far from the root cause. See AUDIT_2026-04-30.md H-11.
    pub fn validate(&self) -> Result<(), GenesisError> {
        if self.chain_id != self.config.chain_id {
            return Err(GenesisError::ChainIdMismatch {
                spec:   self.chain_id,
                config: self.config.chain_id,
            });
        }
        if self.extra_data.len() > 32 {
            return Err(GenesisError::ExtraDataTooLarge(self.extra_data.len()));
        }
        if self.validators.is_empty() {
            return Err(GenesisError::NoValidators);
        }
        // Each alloc balance must be parseable hex; bad entries would silently
        // be treated as zero by `total_allocated` and cause supply drift.
        for (addr, a) in &self.alloc {
            let s = a.balance.trim_start_matches("0x");
            u128::from_str_radix(s, 16).map_err(|_| GenesisError::BadAllocBalance {
                addr:  addr.clone(),
                value: a.balance.clone(),
            })?;
        }
        let total = self.total_allocated();
        // 2^120 wei ≈ 10^36 ZBX — vastly more than any plausible supply.
        if total > (1u128 << 120) {
            return Err(GenesisError::AllocOverflow(total));
        }
        Ok(())
    }
}