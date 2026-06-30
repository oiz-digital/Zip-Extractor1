//! ZVM state snapshot and revert -- essential for CALL/CREATE isolation.
//!
//! When the ZVM executes a CALL or CREATE:
//!   1. Take a state snapshot (save current account states)
//!   2. Execute the inner call / contract creation
//!   3a. If success: commit (discard snapshot)
//!   3b. If REVERT or exception: restore snapshot, return revert data
//!
//! Snapshot granularity:
//!   - Account balance, nonce, code, storage diffs
//!   - Access list (warm/cold accounts, warm/cold storage slots)
//!   - Log buffer (revert clears logs added in failed call)
//!   - Self-destruct list (revert un-schedules self-destructs)
//!
//! EIP-3651 (Shanghai): Warm coinbase
//!   The block proposer's coinbase address is pre-warmed at block start.
//!   Before EIP-3651, accessing coinbase cost 2600 gas (cold).
//!   After EIP-3651, coinbase costs 100 gas (warm) from the first access.
//!   ZBX implements EIP-3651 as of block_number >= SHANGHAI_BLOCK.
//!
//! Nested calls: ZVM supports up to 1024 call stack depth.
//! Each level of nesting gets its own snapshot.

use std::collections::HashMap;

/// EIP-3651 (Shanghai) -- warm coinbase activation block.
pub const SHANGHAI_BLOCK: u64 = 0; // Active from genesis on ZBX

// ── Access list (EIP-2929 / EIP-2930) ────────────────────────────────────────

/// Per-transaction access list tracking warm/cold accounts and storage.
///
/// Warm = already accessed this tx (100 gas)
/// Cold = first access this tx    (2600 gas for accounts, 2100 for storage)
#[derive(Debug, Clone, Default)]
pub struct AccessListState {
    /// Warm accounts (address -> true if warm)
    pub warm_accounts: HashMap<[u8; 20], bool>,
    /// Warm storage slots (address, slot) -> true if warm
    pub warm_storage:  HashMap<([u8; 20], [u8; 32]), bool>,
}

impl AccessListState {
    pub fn new() -> Self { Self::default() }

    /// Pre-warm addresses required at tx start (EIP-2929 rules).
    /// Includes: tx.from, tx.to (or create address), precompiles, coinbase (EIP-3651).
    pub fn warm_tx_start(
        &mut self,
        from:     [u8; 20],
        to:       Option<[u8; 20]>,
        coinbase: [u8; 20],
        block:    u64,
        precompiles: &[[u8; 20]],
    ) {
        // Always warm: sender, recipient, precompiles
        self.warm_accounts.insert(from, true);
        if let Some(to) = to { self.warm_accounts.insert(to, true); }
        for &pre in precompiles { self.warm_accounts.insert(pre, true); }
        // EIP-3651: warm coinbase from block >= SHANGHAI_BLOCK
        if block >= SHANGHAI_BLOCK {
            self.warm_coinbase(coinbase);
        }
    }

    /// EIP-3651: Pre-warm the block proposer coinbase address.
    /// After this call, all CALL/BALANCE/etc to coinbase cost 100 gas (warm).
    pub fn warm_coinbase(&mut self, coinbase: [u8; 20]) {
        self.warm_accounts.insert(coinbase, true);
    }

    pub fn is_warm_account(&self, addr: &[u8; 20]) -> bool {
        self.warm_accounts.get(addr).copied().unwrap_or(false)
    }

    pub fn mark_warm_account(&mut self, addr: [u8; 20]) -> bool {
        let was_warm = self.is_warm_account(&addr);
        self.warm_accounts.insert(addr, true);
        was_warm // true if already warm (cheap access)
    }

    pub fn is_warm_storage(&self, addr: &[u8; 20], slot: &[u8; 32]) -> bool {
        self.warm_storage.get(&(*addr, *slot)).copied().unwrap_or(false)
    }

    pub fn mark_warm_storage(&mut self, addr: [u8; 20], slot: [u8; 32]) -> bool {
        let was_warm = self.is_warm_storage(&addr, &slot);
        self.warm_storage.insert((addr, slot), true);
        was_warm
    }
}

// ── State snapshot ────────────────────────────────────────────────────────────

/// A point-in-time snapshot of ZVM state for a single call frame.
/// Created before each CALL/CREATE; restored on REVERT or exception.
#[derive(Debug, Clone)]
pub struct StateSnapshot {
    /// Snapshot ID (monotonically increasing)
    pub id:              u64,
    /// Modified accounts at snapshot time (address -> pre-snapshot state)
    pub account_reverts: HashMap<[u8; 20], AccountRevert>,
    /// Storage reverts (address, slot) -> pre-snapshot value
    pub storage_reverts: HashMap<([u8; 20], [u8; 32]), [u8; 32]>,
    /// Access list snapshot (restore warm/cold state on revert)
    pub access_list:     AccessListState,
    /// Number of logs at snapshot time (revert trims log buffer)
    pub log_count:       usize,
    /// Self-destruct queue at snapshot time
    pub selfdestruct_count: usize,
    /// Gas remaining at snapshot
    pub gas_remaining:   u64,
}

/// Account state at snapshot time (for rollback).
#[derive(Debug, Clone)]
pub struct AccountRevert {
    pub balance: u128,
    pub nonce:   u64,
    pub code:    Option<Vec<u8>>,
    pub exists:  bool,  // false if account was created during this call
}

/// ZVM state with snapshot stack.
pub struct ZvmState {
    /// Stack of active snapshots (innermost call first)
    pub snapshots:       Vec<StateSnapshot>,
    pub next_snapshot_id: u64,
    /// Access list for current tx
    pub access_list:     AccessListState,
    /// Log buffer for current tx (cleared per tx, trimmed on REVERT)
    pub log_buffer:      Vec<TxLog>,
    /// Accounts modified this tx (dirty set)
    pub dirty_accounts:  HashMap<[u8; 20], DirtyAccount>,
    /// Self-destruct queue (executed at block end)
    pub selfdestructs:   Vec<([u8; 20], [u8; 20])>, // (who, beneficiary)
}

#[derive(Debug, Clone)]
pub struct TxLog {
    pub address: [u8; 20],
    pub topics:  Vec<[u8; 32]>,
    pub data:    Vec<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct DirtyAccount {
    pub balance:  u128,
    pub nonce:    u64,
    pub code:     Option<Vec<u8>>,
    pub storage:  HashMap<[u8; 32], [u8; 32]>,
    pub created:  bool,
    pub deleted:  bool,
}

impl ZvmState {
    pub fn new() -> Self {
        Self {
            snapshots:        Vec::new(),
            next_snapshot_id: 0,
            access_list:      AccessListState::new(),
            log_buffer:       Vec::new(),
            dirty_accounts:   HashMap::new(),
            selfdestructs:    Vec::new(),
        }
    }

    /// Take a snapshot before a CALL or CREATE (EIP-1283 style).
    /// Returns the snapshot ID for later revert_snapshot() or commit_snapshot().
    pub fn take_snapshot(&mut self, gas_remaining: u64) -> u64 {
        let id = self.next_snapshot_id;
        self.next_snapshot_id += 1;
        let snap = StateSnapshot {
            id,
            account_reverts:     HashMap::new(),
            storage_reverts:     HashMap::new(),
            access_list:         self.access_list.clone(),
            log_count:           self.log_buffer.len(),
            selfdestruct_count:  self.selfdestructs.len(),
            gas_remaining,
        };
        self.snapshots.push(snap);
        id
    }

    /// Revert state to a snapshot (on REVERT opcode or exception).
    /// Restores: accounts, storage, access list, logs, self-destructs.
    pub fn revert_snapshot(&mut self, snapshot_id: u64) {
        if let Some(pos) = self.snapshots.iter().rposition(|s| s.id == snapshot_id) {
            let snap = self.snapshots.remove(pos);
            // Restore access list
            self.access_list = snap.access_list;
            // Trim log buffer to pre-call state
            self.log_buffer.truncate(snap.log_count);
            // Undo self-destructs scheduled in failed call
            self.selfdestructs.truncate(snap.selfdestruct_count);
            // Revert account and storage changes
            for (addr, revert) in snap.account_reverts {
                if let Some(acc) = self.dirty_accounts.get_mut(&addr) {
                    acc.balance = revert.balance;
                    acc.nonce   = revert.nonce;
                    acc.code    = revert.code;
                    if !revert.exists { acc.deleted = true; }
                }
            }
            for ((addr, slot), old_val) in snap.storage_reverts {
                if let Some(acc) = self.dirty_accounts.get_mut(&addr) {
                    acc.storage.insert(slot, old_val);
                }
            }
        }
    }

    /// Commit a snapshot (call succeeded -- discard the snapshot, keep changes).
    pub fn commit_snapshot(&mut self, snapshot_id: u64) {
        self.snapshots.retain(|s| s.id != snapshot_id);
    }

    /// Emit a log (from LOG0-LOG4 opcodes).
    pub fn emit_log(&mut self, log: TxLog) { self.log_buffer.push(log); }

    /// Schedule SELFDESTRUCT (executed at end of block in EIP-6780 mode).
    pub fn schedule_selfdestruct(&mut self, who: [u8; 20], beneficiary: [u8; 20]) {
        self.selfdestructs.push((who, beneficiary));
    }
}