//! Deterministic execution primitives.
//!
//! This module is the **type-and-codec layer** of the deterministic execution
//! contract. Concrete dispatch (opcode metering, persistence, gas refunds at
//! tx end) lives in `zbx-vm` / `zbx-state` / `zbx-consensus` and consumes
//! these types through serde/RLP.
//!
//! Design discipline (matches `module_version.rs`, `governance.rs`,
//! `version_registry.rs`):
//!
//! 1. **Pure data + invariants.** Every constructor validates *all* fields;
//!    every decoder re-runs the same `validate()` so a maliciously-encoded
//!    value cannot bypass invariants by skipping the constructor.
//! 2. **`BTreeMap` for any keyed map** so RLP byte-output is canonical and
//!    independent of insertion order.
//! 3. **`inner.rlp_append(s)` for newtype `Encodable` delegation** — never
//!    `s.append(&inner)` (LESSON #11: doubles `note_appended(1)` and breaks
//!    the parent `begin_list(N)` contract).
//! 4. **Field-count gate (`item_count() != N`)** at the top of every
//!    `Decodable::decode` to refuse short/long lists with `RlpIncorrectListLen`.
//! 5. **Network-agnostic.** No `chain_id` literal here — the chain ID enters
//!    `ExecutionContext` from the caller (consensus loop) so the same code
//!    serves mainnet (8989), testnet (8990), and devnet (8990).

use std::collections::BTreeMap;

use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};
use serde::{Deserialize, Serialize};

use crate::address::Address;

// ---------------------------------------------------------------------------
// ExecutionLimits — chain-wide caps on a single block / single tx.
// ---------------------------------------------------------------------------

/// Hard limits the execution engine MUST refuse to exceed.
///
/// All fields are *post-genesis* parameters, but in practice they only change
/// via a `RegistryUpgrade` governance proposal so they behave as constants
/// for the lifetime of a fork.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExecutionLimits {
    /// Maximum gas a single block may consume (sum across all txs).
    pub max_block_gas: u64,
    /// Maximum gas a single transaction may consume.
    pub max_tx_gas: u64,
    /// Maximum byte size of an encoded block (header + body).
    pub max_block_size: u32,
    /// Maximum byte size of an encoded signed transaction.
    pub max_tx_size: u32,
    /// Maximum nested call depth (CALL, DELEGATECALL, STATICCALL, CREATE).
    pub max_call_depth: u16,
    /// Maximum number of LOG entries a single tx may emit.
    pub max_logs_per_tx: u32,
    /// Maximum total bytes across all log topics+data of a single tx.
    pub max_log_size: u32,
}

impl ExecutionLimits {
    /// Recommended mainnet defaults (8989). Conservative until benchmarked.
    pub const MAINNET_DEFAULT: Self = Self {
        max_block_gas: 30_000_000,
        max_tx_gas: 15_000_000,
        max_block_size: 2 * 1024 * 1024, // 2 MiB
        max_tx_size: 128 * 1024,         // 128 KiB
        max_call_depth: 1024,
        max_logs_per_tx: 256,
        max_log_size: 64 * 1024, // 64 KiB
    };

    /// Recommended testnet/devnet defaults (8990). Slightly looser to permit
    /// stress testing without governance proposals.
    pub const TESTNET_DEFAULT: Self = Self {
        max_block_gas: 60_000_000,
        max_tx_gas: 30_000_000,
        max_block_size: 4 * 1024 * 1024,
        max_tx_size: 256 * 1024,
        max_call_depth: 1024,
        max_logs_per_tx: 512,
        max_log_size: 128 * 1024,
    };

    /// Validate cross-field invariants. Called by both the constructor path
    /// and by `Decodable::decode` so RLP and serde inputs cannot bypass.
    pub fn validate(&self) -> Result<(), DecoderError> {
        if self.max_block_gas == 0 {
            return Err(DecoderError::Custom("max_block_gas must be > 0"));
        }
        if self.max_tx_gas == 0 {
            return Err(DecoderError::Custom("max_tx_gas must be > 0"));
        }
        if self.max_tx_gas > self.max_block_gas {
            return Err(DecoderError::Custom(
                "max_tx_gas must not exceed max_block_gas",
            ));
        }
        if self.max_block_size == 0 {
            return Err(DecoderError::Custom("max_block_size must be > 0"));
        }
        if self.max_tx_size == 0 {
            return Err(DecoderError::Custom("max_tx_size must be > 0"));
        }
        if self.max_tx_size as u64 > self.max_block_size as u64 {
            return Err(DecoderError::Custom(
                "max_tx_size must not exceed max_block_size",
            ));
        }
        if self.max_call_depth == 0 {
            return Err(DecoderError::Custom("max_call_depth must be > 0"));
        }
        Ok(())
    }

    /// Build a validated `ExecutionLimits`.
    pub fn new(
        max_block_gas: u64,
        max_tx_gas: u64,
        max_block_size: u32,
        max_tx_size: u32,
        max_call_depth: u16,
        max_logs_per_tx: u32,
        max_log_size: u32,
    ) -> Result<Self, DecoderError> {
        let s = Self {
            max_block_gas,
            max_tx_gas,
            max_block_size,
            max_tx_size,
            max_call_depth,
            max_logs_per_tx,
            max_log_size,
        };
        s.validate()?;
        Ok(s)
    }
}

impl Encodable for ExecutionLimits {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(7);
        s.append(&self.max_block_gas);
        s.append(&self.max_tx_gas);
        s.append(&self.max_block_size);
        s.append(&self.max_tx_size);
        s.append(&self.max_call_depth);
        s.append(&self.max_logs_per_tx);
        s.append(&self.max_log_size);
    }
}

impl Decodable for ExecutionLimits {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if rlp.item_count()? != 7 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        let s = Self {
            max_block_gas: rlp.val_at(0)?,
            max_tx_gas: rlp.val_at(1)?,
            max_block_size: rlp.val_at(2)?,
            max_tx_size: rlp.val_at(3)?,
            max_call_depth: rlp.val_at(4)?,
            max_logs_per_tx: rlp.val_at(5)?,
            max_log_size: rlp.val_at(6)?,
        };
        s.validate()?;
        Ok(s)
    }
}

// ---------------------------------------------------------------------------
// GasMeter — deterministic gas accounting for a single tx.
// ---------------------------------------------------------------------------

/// Reasons a `GasMeter` operation can fail.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GasError {
    /// Caller asked to consume more gas than is available.
    OutOfGas { requested: u64, remaining: u64 },
    /// Caller tried to refund more than has been used.
    RefundExceedsUsed { requested: u64, used: u64 },
    /// Snapshot stack underflow on `restore`.
    NoSnapshot,
}

impl std::fmt::Display for GasError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OutOfGas {
                requested,
                remaining,
            } => write!(f, "out of gas: requested {requested}, remaining {remaining}"),
            Self::RefundExceedsUsed { requested, used } => {
                write!(f, "refund {requested} exceeds used {used}")
            }
            Self::NoSnapshot => write!(f, "no gas snapshot to restore"),
        }
    }
}

impl std::error::Error for GasError {}

/// Deterministic gas meter.
///
/// Holds `(remaining, used, refund)` plus a snapshot stack for nested calls.
/// `consume`/`refund` are the only mutators; both update the running totals
/// atomically so a failed `consume` does **not** alter state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GasMeter {
    initial: u64,
    remaining: u64,
    used: u64,
    refund: u64,
    #[serde(skip)]
    snapshots: Vec<(u64, u64, u64)>, // (remaining, used, refund)
}

impl GasMeter {
    /// Build a meter with `gas_limit` units available.
    pub fn new(gas_limit: u64) -> Self {
        Self {
            initial: gas_limit,
            remaining: gas_limit,
            used: 0,
            refund: 0,
            snapshots: Vec::new(),
        }
    }

    /// Total budget at construction time.
    pub fn initial(&self) -> u64 {
        self.initial
    }

    /// Gas not yet consumed.
    pub fn remaining(&self) -> u64 {
        self.remaining
    }

    /// Gas consumed so far (does NOT subtract refund).
    pub fn used(&self) -> u64 {
        self.used
    }

    /// Pending refund accumulated from `SSTORE`-clear style ops.
    pub fn refund(&self) -> u64 {
        self.refund
    }

    /// Net gas the caller will be billed (used minus capped refund).
    /// EIP-3529 caps refund at `used / 5`. Match that here.
    pub fn billed(&self) -> u64 {
        let cap = self.used / 5;
        let effective_refund = self.refund.min(cap);
        self.used.saturating_sub(effective_refund)
    }

    /// Subtract `cost` from `remaining`, add to `used`. Atomic on failure.
    pub fn consume(&mut self, cost: u64) -> Result<(), GasError> {
        if cost > self.remaining {
            return Err(GasError::OutOfGas {
                requested: cost,
                remaining: self.remaining,
            });
        }
        self.remaining -= cost;
        self.used += cost;
        Ok(())
    }

    /// Add `amount` to the refund counter. Refund is bounded by `used` to
    /// prevent overflow attacks and matches EVM semantics.
    pub fn add_refund(&mut self, amount: u64) -> Result<(), GasError> {
        let new_total = self.refund.saturating_add(amount);
        if new_total > self.used {
            return Err(GasError::RefundExceedsUsed {
                requested: new_total,
                used: self.used,
            });
        }
        self.refund = new_total;
        Ok(())
    }

    /// Push a snapshot onto the call stack. Use before nested CALL.
    pub fn snapshot(&mut self) {
        self.snapshots.push((self.remaining, self.used, self.refund));
    }

    /// Pop the most recent snapshot and restore counters. Use on REVERT.
    pub fn restore(&mut self) -> Result<(), GasError> {
        let (r, u, rf) = self.snapshots.pop().ok_or(GasError::NoSnapshot)?;
        self.remaining = r;
        self.used = u;
        self.refund = rf;
        Ok(())
    }

    /// Discard the most recent snapshot, keeping current counters. Use on
    /// successful nested CALL return.
    pub fn commit(&mut self) -> Result<(), GasError> {
        self.snapshots.pop().ok_or(GasError::NoSnapshot)?;
        Ok(())
    }

    /// Snapshot stack depth (informational; consensus uses
    /// `ExecutionLimits::max_call_depth` instead).
    pub fn depth(&self) -> usize {
        self.snapshots.len()
    }
}

impl Encodable for GasMeter {
    fn rlp_append(&self, s: &mut RlpStream) {
        // Only persist the wire-stable triple. Snapshots are a runtime stack
        // and never cross block / network boundaries.
        s.begin_list(4);
        s.append(&self.initial);
        s.append(&self.remaining);
        s.append(&self.used);
        s.append(&self.refund);
    }
}

impl Decodable for GasMeter {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if rlp.item_count()? != 4 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        let initial: u64 = rlp.val_at(0)?;
        let remaining: u64 = rlp.val_at(1)?;
        let used: u64 = rlp.val_at(2)?;
        let refund: u64 = rlp.val_at(3)?;
        if remaining > initial {
            return Err(DecoderError::Custom("remaining exceeds initial"));
        }
        if used > initial {
            return Err(DecoderError::Custom("used exceeds initial"));
        }
        if remaining + used != initial {
            return Err(DecoderError::Custom(
                "remaining + used must equal initial",
            ));
        }
        if refund > used {
            return Err(DecoderError::Custom("refund exceeds used"));
        }
        Ok(Self {
            initial,
            remaining,
            used,
            refund,
            snapshots: Vec::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// DeterministicClock — block-timestamp + monotonicity invariant.
// ---------------------------------------------------------------------------

/// Source of "now" for a block. Always derived from the proposed block
/// header — never from the wall clock — so all validators agree.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DeterministicClock {
    /// Block timestamp in **milliseconds** since Unix epoch.
    pub block_timestamp_ms: u64,
    /// Strictly-greater-than-zero block number; genesis is `1`.
    pub block_number: u64,
}

impl DeterministicClock {
    /// Build a validated clock.
    pub fn new(block_number: u64, block_timestamp_ms: u64) -> Result<Self, DecoderError> {
        let s = Self {
            block_timestamp_ms,
            block_number,
        };
        s.validate()?;
        Ok(s)
    }

    /// Validate that `block_number > 0` (genesis = 1) and timestamp > 0.
    pub fn validate(&self) -> Result<(), DecoderError> {
        if self.block_number == 0 {
            return Err(DecoderError::Custom("block_number must be > 0"));
        }
        if self.block_timestamp_ms == 0 {
            return Err(DecoderError::Custom("block_timestamp_ms must be > 0"));
        }
        Ok(())
    }

    /// Return Ok if `next` is strictly after `self`.
    /// Consensus rule: every block's timestamp MUST be > parent's timestamp.
    pub fn ensure_monotone_strict(&self, next: &Self) -> Result<(), DecoderError> {
        if next.block_number <= self.block_number {
            return Err(DecoderError::Custom(
                "block_number must strictly increase",
            ));
        }
        if next.block_timestamp_ms <= self.block_timestamp_ms {
            return Err(DecoderError::Custom(
                "block_timestamp_ms must strictly increase",
            ));
        }
        Ok(())
    }
}

impl Encodable for DeterministicClock {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(2);
        s.append(&self.block_number);
        s.append(&self.block_timestamp_ms);
    }
}

impl Decodable for DeterministicClock {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if rlp.item_count()? != 2 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        let s = Self {
            block_number: rlp.val_at(0)?,
            block_timestamp_ms: rlp.val_at(1)?,
        };
        s.validate()?;
        Ok(s)
    }
}

// ---------------------------------------------------------------------------
// ExecutionContext — per-block execution environment.
// ---------------------------------------------------------------------------

/// Frozen, deterministic environment for executing one block's worth of txs.
///
/// Built once per block by consensus and handed to the VM. All non-deterministic
/// inputs (wall clock, OS RNG, network state) are explicitly excluded.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExecutionContext {
    /// Network identifier (8989 mainnet, 8990 testnet+devnet).
    pub chain_id: u64,
    /// Block number being executed.
    pub block_number: u64,
    /// Block timestamp in ms since Unix epoch.
    pub block_timestamp_ms: u64,
    /// Address of the validator that proposed this block.
    pub proposer: Address,
    /// Total gas allowed across all txs in this block.
    pub block_gas_limit: u64,
    /// Per-block deterministic seed (e.g. `keccak(parent_hash || proposer)`).
    /// All RNG-using opcodes derive from this.
    pub randomness_seed: [u8; 32],
    /// Hard limits enforced for the entire block.
    pub limits: ExecutionLimits,
}

impl ExecutionContext {
    /// Build a validated context.
    pub fn new(
        chain_id: u64,
        block_number: u64,
        block_timestamp_ms: u64,
        proposer: Address,
        block_gas_limit: u64,
        randomness_seed: [u8; 32],
        limits: ExecutionLimits,
    ) -> Result<Self, DecoderError> {
        let s = Self {
            chain_id,
            block_number,
            block_timestamp_ms,
            proposer,
            block_gas_limit,
            randomness_seed,
            limits,
        };
        s.validate()?;
        Ok(s)
    }

    /// Validate all invariants. Re-run on decode.
    pub fn validate(&self) -> Result<(), DecoderError> {
        if self.chain_id == 0 {
            return Err(DecoderError::Custom("chain_id must be > 0"));
        }
        if self.block_number == 0 {
            return Err(DecoderError::Custom("block_number must be > 0"));
        }
        if self.block_timestamp_ms == 0 {
            return Err(DecoderError::Custom("block_timestamp_ms must be > 0"));
        }
        if self.block_gas_limit == 0 {
            return Err(DecoderError::Custom("block_gas_limit must be > 0"));
        }
        if self.block_gas_limit > self.limits.max_block_gas {
            return Err(DecoderError::Custom(
                "block_gas_limit exceeds limits.max_block_gas",
            ));
        }
        self.limits.validate()?;
        Ok(())
    }

    /// Convenience accessor.
    pub fn clock(&self) -> DeterministicClock {
        DeterministicClock {
            block_number: self.block_number,
            block_timestamp_ms: self.block_timestamp_ms,
        }
    }
}

// Local helper: 20-byte raw address payload as a single RLP byte-string.
// Crate-private (LESSON #12 — promote to `zbx-codec` only when ≥2 callers).
fn append_address(s: &mut RlpStream, a: &Address) {
    s.append(&a.0.as_ref());
}

fn decode_address(rlp: &Rlp) -> Result<Address, DecoderError> {
    let bytes: Vec<u8> = rlp.as_val()?;
    if bytes.len() != 20 {
        return Err(DecoderError::Custom("address must be 20 bytes"));
    }
    let mut out = [0u8; 20];
    out.copy_from_slice(&bytes);
    Ok(Address(out))
}

impl Encodable for ExecutionContext {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(7);
        s.append(&self.chain_id);
        s.append(&self.block_number);
        s.append(&self.block_timestamp_ms);
        append_address(s, &self.proposer);
        s.append(&self.block_gas_limit);
        // randomness_seed as a single 32-byte byte-string.
        s.append(&self.randomness_seed.as_ref());
        // Composite — direct delegation per LESSON #11.
        self.limits.rlp_append(s);
    }
}

impl Decodable for ExecutionContext {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if rlp.item_count()? != 7 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        let chain_id: u64 = rlp.val_at(0)?;
        let block_number: u64 = rlp.val_at(1)?;
        let block_timestamp_ms: u64 = rlp.val_at(2)?;
        let proposer = decode_address(&rlp.at(3)?)?;
        let block_gas_limit: u64 = rlp.val_at(4)?;
        let seed_bytes: Vec<u8> = rlp.val_at(5)?;
        if seed_bytes.len() != 32 {
            return Err(DecoderError::Custom("randomness_seed must be 32 bytes"));
        }
        let mut randomness_seed = [0u8; 32];
        randomness_seed.copy_from_slice(&seed_bytes);
        let limits = ExecutionLimits::decode(&rlp.at(6)?)?;
        let s = Self {
            chain_id,
            block_number,
            block_timestamp_ms,
            proposer,
            block_gas_limit,
            randomness_seed,
            limits,
        };
        s.validate()?;
        Ok(s)
    }
}

// ---------------------------------------------------------------------------
// BlockGasTracker — running gas consumption across a block.
// ---------------------------------------------------------------------------

/// Tracks cumulative gas consumed across all txs in a single block.
/// Consensus rejects a block whose `total_gas_used > block_gas_limit`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BlockGasTracker {
    block_gas_limit: u64,
    total_gas_used: u64,
    /// Per-tx-index → gas billed. Sorted-strict-monotone on decode.
    per_tx: BTreeMap<u32, u64>,
}

impl BlockGasTracker {
    pub fn new(block_gas_limit: u64) -> Self {
        Self {
            block_gas_limit,
            total_gas_used: 0,
            per_tx: BTreeMap::new(),
        }
    }

    pub fn block_gas_limit(&self) -> u64 {
        self.block_gas_limit
    }

    pub fn total_gas_used(&self) -> u64 {
        self.total_gas_used
    }

    pub fn remaining(&self) -> u64 {
        self.block_gas_limit.saturating_sub(self.total_gas_used)
    }

    pub fn per_tx(&self) -> &BTreeMap<u32, u64> {
        &self.per_tx
    }

    /// Record `gas_billed` for tx at `tx_index`. Refuses to exceed the block
    /// limit and refuses duplicate `tx_index`.
    pub fn record(&mut self, tx_index: u32, gas_billed: u64) -> Result<(), GasError> {
        if self.per_tx.contains_key(&tx_index) {
            return Err(GasError::OutOfGas {
                requested: gas_billed,
                remaining: 0,
            });
        }
        let new_total = self.total_gas_used.saturating_add(gas_billed);
        if new_total > self.block_gas_limit {
            return Err(GasError::OutOfGas {
                requested: gas_billed,
                remaining: self.remaining(),
            });
        }
        self.total_gas_used = new_total;
        self.per_tx.insert(tx_index, gas_billed);
        Ok(())
    }
}

impl Encodable for BlockGasTracker {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(3);
        s.append(&self.block_gas_limit);
        s.append(&self.total_gas_used);
        s.begin_list(self.per_tx.len());
        for (idx, gas) in &self.per_tx {
            s.begin_list(2);
            s.append(idx);
            s.append(gas);
        }
    }
}

impl Decodable for BlockGasTracker {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if rlp.item_count()? != 3 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        let block_gas_limit: u64 = rlp.val_at(0)?;
        let total_gas_used: u64 = rlp.val_at(1)?;
        let rows = rlp.at(2)?;
        let mut per_tx = BTreeMap::new();
        let mut sum: u64 = 0;
        let mut prev: Option<u32> = None;
        for item in rows.iter() {
            if item.item_count()? != 2 {
                return Err(DecoderError::RlpIncorrectListLen);
            }
            let idx: u32 = item.val_at(0)?;
            let gas: u64 = item.val_at(1)?;
            if let Some(p) = prev {
                if idx <= p {
                    return Err(DecoderError::Custom(
                        "per_tx must be strictly monotone by tx_index",
                    ));
                }
            }
            prev = Some(idx);
            sum = sum.saturating_add(gas);
            per_tx.insert(idx, gas);
        }
        if sum != total_gas_used {
            return Err(DecoderError::Custom(
                "per_tx sum must equal total_gas_used",
            ));
        }
        if total_gas_used > block_gas_limit {
            return Err(DecoderError::Custom(
                "total_gas_used exceeds block_gas_limit",
            ));
        }
        Ok(Self {
            block_gas_limit,
            total_gas_used,
            per_tx,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rlp::{decode, encode};

    fn addr(n: u8) -> Address {
        Address([n; 20])
    }

    // ----- ExecutionLimits -----

    #[test]
    fn limits_mainnet_default_validates() {
        ExecutionLimits::MAINNET_DEFAULT.validate().unwrap();
    }

    #[test]
    fn limits_testnet_default_validates() {
        ExecutionLimits::TESTNET_DEFAULT.validate().unwrap();
    }

    #[test]
    fn limits_rejects_tx_gas_above_block_gas() {
        let r = ExecutionLimits::new(10, 20, 100, 50, 16, 8, 1024);
        assert!(matches!(r, Err(DecoderError::Custom(_))));
    }

    #[test]
    fn limits_rejects_zero_block_gas() {
        let r = ExecutionLimits::new(0, 0, 100, 50, 16, 8, 1024);
        assert!(matches!(r, Err(DecoderError::Custom(_))));
    }

    #[test]
    fn limits_rejects_tx_size_above_block_size() {
        let r = ExecutionLimits::new(100, 50, 50, 100, 16, 8, 1024);
        assert!(matches!(r, Err(DecoderError::Custom(_))));
    }

    #[test]
    fn limits_rlp_round_trip() {
        let l = ExecutionLimits::MAINNET_DEFAULT;
        let bytes = encode(&l);
        let back: ExecutionLimits = decode(&bytes).unwrap();
        assert_eq!(l, back);
    }

    #[test]
    fn limits_decode_rejects_wrong_field_count() {
        let mut s = RlpStream::new_list(6);
        s.append(&100u64);
        s.append(&50u64);
        s.append(&100u32);
        s.append(&50u32);
        s.append(&16u16);
        s.append(&8u32);
        let bytes = s.out();
        let r: Result<ExecutionLimits, _> = decode(&bytes);
        assert!(matches!(r, Err(DecoderError::RlpIncorrectListLen)));
    }

    // ----- GasMeter -----

    #[test]
    fn gas_meter_consume_decrements_remaining() {
        let mut g = GasMeter::new(100);
        g.consume(30).unwrap();
        assert_eq!(g.remaining(), 70);
        assert_eq!(g.used(), 30);
    }

    #[test]
    fn gas_meter_consume_atomic_on_failure() {
        let mut g = GasMeter::new(100);
        let r = g.consume(101);
        assert!(matches!(r, Err(GasError::OutOfGas { .. })));
        // State unchanged.
        assert_eq!(g.remaining(), 100);
        assert_eq!(g.used(), 0);
    }

    #[test]
    fn gas_meter_refund_capped_by_used() {
        let mut g = GasMeter::new(100);
        g.consume(50).unwrap();
        let r = g.add_refund(60);
        assert!(matches!(r, Err(GasError::RefundExceedsUsed { .. })));
    }

    #[test]
    fn gas_meter_billed_caps_refund_at_one_fifth() {
        let mut g = GasMeter::new(100);
        g.consume(50).unwrap();
        // Try to refund half — capped at used/5 = 10.
        g.add_refund(25).unwrap();
        assert_eq!(g.billed(), 50 - 10);
    }

    #[test]
    fn gas_meter_snapshot_restore_unwinds() {
        let mut g = GasMeter::new(100);
        g.consume(20).unwrap();
        g.snapshot();
        g.consume(30).unwrap();
        assert_eq!(g.remaining(), 50);
        g.restore().unwrap();
        assert_eq!(g.remaining(), 80);
        assert_eq!(g.used(), 20);
    }

    #[test]
    fn gas_meter_commit_keeps_state_pops_snapshot() {
        let mut g = GasMeter::new(100);
        g.snapshot();
        g.consume(10).unwrap();
        g.commit().unwrap();
        assert_eq!(g.remaining(), 90);
        assert_eq!(g.depth(), 0);
    }

    #[test]
    fn gas_meter_restore_underflow_errors() {
        let mut g = GasMeter::new(100);
        let r = g.restore();
        assert!(matches!(r, Err(GasError::NoSnapshot)));
    }

    #[test]
    fn gas_meter_rlp_round_trip() {
        let mut g = GasMeter::new(100);
        g.consume(30).unwrap();
        g.add_refund(5).unwrap();
        let bytes = encode(&g);
        let back: GasMeter = decode(&bytes).unwrap();
        assert_eq!(back.remaining(), 70);
        assert_eq!(back.used(), 30);
        assert_eq!(back.refund(), 5);
        assert_eq!(back.initial(), 100);
        // Snapshots not persisted.
        assert_eq!(back.depth(), 0);
    }

    #[test]
    fn gas_meter_decode_rejects_remaining_plus_used_mismatch() {
        let mut s = RlpStream::new_list(4);
        s.append(&100u64); // initial
        s.append(&50u64); // remaining
        s.append(&30u64); // used (50+30 != 100)
        s.append(&0u64);
        let bytes = s.out();
        let r: Result<GasMeter, _> = decode(&bytes);
        assert!(matches!(r, Err(DecoderError::Custom(_))));
    }

    // ----- DeterministicClock -----

    #[test]
    fn clock_validates_nonzero() {
        assert!(DeterministicClock::new(0, 1000).is_err());
        assert!(DeterministicClock::new(1, 0).is_err());
        DeterministicClock::new(1, 1000).unwrap();
    }

    #[test]
    fn clock_monotone_strict_accepts_advance() {
        let a = DeterministicClock::new(1, 1000).unwrap();
        let b = DeterministicClock::new(2, 1500).unwrap();
        a.ensure_monotone_strict(&b).unwrap();
    }

    #[test]
    fn clock_monotone_strict_rejects_equal_block_number() {
        let a = DeterministicClock::new(2, 1000).unwrap();
        let b = DeterministicClock::new(2, 2000).unwrap();
        assert!(a.ensure_monotone_strict(&b).is_err());
    }

    #[test]
    fn clock_monotone_strict_rejects_equal_timestamp() {
        let a = DeterministicClock::new(1, 1000).unwrap();
        let b = DeterministicClock::new(2, 1000).unwrap();
        assert!(a.ensure_monotone_strict(&b).is_err());
    }

    #[test]
    fn clock_rlp_round_trip() {
        let c = DeterministicClock::new(42, 1234567).unwrap();
        let bytes = encode(&c);
        let back: DeterministicClock = decode(&bytes).unwrap();
        assert_eq!(c, back);
    }

    // ----- ExecutionContext -----

    #[test]
    fn ctx_validates_chain_id_nonzero() {
        let r = ExecutionContext::new(
            0,
            1,
            1000,
            addr(1),
            10_000,
            [0u8; 32],
            ExecutionLimits::MAINNET_DEFAULT,
        );
        assert!(r.is_err());
    }

    #[test]
    fn ctx_validates_block_gas_limit_within_max() {
        let r = ExecutionContext::new(
            8989,
            1,
            1000,
            addr(1),
            ExecutionLimits::MAINNET_DEFAULT.max_block_gas + 1,
            [0u8; 32],
            ExecutionLimits::MAINNET_DEFAULT,
        );
        assert!(r.is_err());
    }

    #[test]
    fn ctx_clock_helper_returns_consistent_clock() {
        let ctx = ExecutionContext::new(
            8989,
            7,
            12345,
            addr(2),
            10_000,
            [1u8; 32],
            ExecutionLimits::MAINNET_DEFAULT,
        )
        .unwrap();
        let clk = ctx.clock();
        assert_eq!(clk.block_number, 7);
        assert_eq!(clk.block_timestamp_ms, 12345);
    }

    #[test]
    fn ctx_rlp_round_trip() {
        let ctx = ExecutionContext::new(
            8989,
            42,
            999_000,
            addr(7),
            5_000_000,
            [9u8; 32],
            ExecutionLimits::MAINNET_DEFAULT,
        )
        .unwrap();
        let bytes = encode(&ctx);
        let back: ExecutionContext = decode(&bytes).unwrap();
        assert_eq!(ctx, back);
    }

    #[test]
    fn ctx_decode_rejects_non_32_byte_seed() {
        let mut s = RlpStream::new_list(7);
        s.append(&8989u64);
        s.append(&1u64);
        s.append(&1000u64);
        s.append(&addr(1).0.as_ref());
        s.append(&10_000u64);
        s.append(&[0u8; 16].as_ref()); // wrong length
        ExecutionLimits::MAINNET_DEFAULT.rlp_append(&mut s);
        let bytes = s.out();
        let r: Result<ExecutionContext, _> = decode(&bytes);
        assert!(matches!(r, Err(DecoderError::Custom(_))));
    }

    // ----- BlockGasTracker -----

    #[test]
    fn tracker_records_within_limit() {
        let mut t = BlockGasTracker::new(1000);
        t.record(0, 200).unwrap();
        t.record(1, 300).unwrap();
        assert_eq!(t.total_gas_used(), 500);
        assert_eq!(t.remaining(), 500);
    }

    #[test]
    fn tracker_rejects_over_limit() {
        let mut t = BlockGasTracker::new(1000);
        t.record(0, 600).unwrap();
        let r = t.record(1, 500);
        assert!(matches!(r, Err(GasError::OutOfGas { .. })));
        assert_eq!(t.total_gas_used(), 600);
    }

    #[test]
    fn tracker_rejects_duplicate_index() {
        let mut t = BlockGasTracker::new(1000);
        t.record(3, 100).unwrap();
        assert!(t.record(3, 50).is_err());
    }

    #[test]
    fn tracker_rlp_round_trip() {
        let mut t = BlockGasTracker::new(1000);
        t.record(0, 100).unwrap();
        t.record(2, 250).unwrap();
        t.record(5, 400).unwrap();
        let bytes = encode(&t);
        let back: BlockGasTracker = decode(&bytes).unwrap();
        assert_eq!(back.total_gas_used(), 750);
        assert_eq!(back.per_tx().len(), 3);
        assert_eq!(back.per_tx().get(&5), Some(&400));
    }

    #[test]
    fn tracker_decode_rejects_unsorted_per_tx() {
        let mut s = RlpStream::new_list(3);
        s.append(&1000u64);
        s.append(&300u64);
        s.begin_list(2);
        s.begin_list(2);
        s.append(&5u32);
        s.append(&100u64);
        s.begin_list(2);
        s.append(&3u32); // out of order
        s.append(&200u64);
        let bytes = s.out();
        let r: Result<BlockGasTracker, _> = decode(&bytes);
        assert!(matches!(r, Err(DecoderError::Custom(_))));
    }

    #[test]
    fn tracker_decode_rejects_sum_mismatch() {
        let mut s = RlpStream::new_list(3);
        s.append(&1000u64);
        s.append(&999u64); // wrong sum
        s.begin_list(1);
        s.begin_list(2);
        s.append(&0u32);
        s.append(&100u64);
        let bytes = s.out();
        let r: Result<BlockGasTracker, _> = decode(&bytes);
        assert!(matches!(r, Err(DecoderError::Custom(_))));
    }
}
