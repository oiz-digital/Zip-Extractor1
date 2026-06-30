//! S32 — Host trait abstraction for the EVM interpreter.
//!
//! The interpreter executes a single call frame in isolation; everything
//! beyond the frame's own memory and stack — account balances, contract
//! code, storage, transactional snapshots — lives in the host. This trait
//! defines the minimal contract the interpreter needs from the outside
//! world to implement CALL/CREATE/SELFDESTRUCT correctly.
//!
//! # Snapshot semantics
//!
//! `snapshot()` returns an opaque `SnapshotId`. The caller may later either
//!   - `commit(id)`     — keep all changes made since `id`, OR
//!   - `revert_to(id)`  — discard all changes made since `id`.
//!
//! Snapshots stack: each CALL/CREATE pushes one. Reverts pop down to and
//! including the named snapshot; commits drop just the named snapshot but
//! keep the writes folded into the parent frame.
//!
//! # Provenance
//!
//! Closes audit C-21 / S7-EVM3 W6 (zbx-evm half). The matching ZvmHost
//! extension in `zbx-zvm` is tracked as W4 in a separate sprint.

use crate::error::EvmError;
use std::collections::HashMap;
use zbx_types::address::Address;

pub type SnapshotId = u64;

/// Minimal interface the EVM interpreter requires from chain state.
///
/// Implementors are responsible for journalling — every mutating call
/// between a `snapshot` and its matching `revert_to` MUST be undone.
pub trait Host {
    fn balance(&self, addr: &Address) -> [u8; 32];
    fn nonce(&self, addr: &Address) -> u64;
    fn code(&self, addr: &Address) -> Vec<u8>;
    fn code_hash(&self, addr: &Address) -> [u8; 32];
    fn storage_load(&self, addr: &Address, key: &[u8; 32]) -> [u8; 32];
    fn storage_store(&mut self, addr: &Address, key: [u8; 32], value: [u8; 32]);
    fn set_code(&mut self, addr: &Address, code: Vec<u8>);
    /// Increment and return the new nonce. Implementations MUST use checked
    /// arithmetic — silently wrapping would make CREATE address derivation
    /// collide with a previous deployment.
    fn inc_nonce(&mut self, addr: &Address) -> Result<u64, EvmError>;
    /// Atomically debit `from` and credit `to`. Returns
    /// `EvmError::InsufficientBalance` if the sender cannot cover `value`.
    fn transfer(&mut self, from: &Address, to: &Address, value: &[u8; 32])
        -> Result<(), EvmError>;
    /// EIP-161: account is "empty" iff nonce==0 AND balance==0 AND code is empty.
    /// Used to assess the GAS_CALL_NEW_ACCOUNT 25 000-gas surcharge.
    fn is_empty(&self, addr: &Address) -> bool;
    /// EIP-6780-aware destruct. Implementations should transfer balance to
    /// `beneficiary` unconditionally, but only delete the code/storage when
    /// `was_created_this_tx(addr)` is true.
    fn destruct(&mut self, addr: &Address, beneficiary: &Address);
    /// Records that `addr` is a contract that was created during the current
    /// transaction. Drives EIP-6780 SELFDESTRUCT semantics.
    fn mark_created_this_tx(&mut self, addr: &Address);
    fn was_created_this_tx(&self, addr: &Address) -> bool;
    fn snapshot(&mut self) -> SnapshotId;
    fn revert_to(&mut self, id: SnapshotId);
    fn commit(&mut self, id: SnapshotId);

    /// Return the keccak256 block hash for the given block number.
    ///
    /// Per the Yellow Paper and EIP-2935, only the 256 most-recent ancestors
    /// of the current block are available; callers (BLOCKHASH opcode handler)
    /// are responsible for enforcing the range guard.  The default returns the
    /// zero hash, which is the correct answer for any block not in the recent
    /// history — production hosts override this to read from their block-hash
    /// ring buffer.
    fn block_hash(&self, block_number: u64) -> [u8; 32] {
        let _ = block_number;
        [0u8; 32]
    }

    /// Task #3 (Precompile 0x0A — PayID resolution): forward `name → address`
    /// resolver. Default `None` keeps existing test hosts compiling; the
    /// production EVM host (when wired) overrides this to read the chain's
    /// PayID registrar storage. The 0x0A precompile observes `None` as the
    /// canonical "unregistered → address(0)" reply, NOT as a revert.
    fn resolve_pay_id_bytes(&self, name: &[u8]) -> Option<[u8; 20]> {
        let _ = name;
        None
    }

    /// Task #3 (Precompile 0x0A): reverse `address → name` lookup. Default
    /// `None` mirrors `resolve_pay_id_bytes`; an unregistered address is
    /// observed as an empty string, not a revert.
    fn reverse_pay_id(&self, addr: &[u8; 20]) -> Option<Vec<u8>> {
        let _ = addr;
        None
    }
}

// ---------------------------------------------------------------------------
//  MockHost — in-memory implementation for tests and offline tooling.
// ---------------------------------------------------------------------------

/// Per-account state snapshot used by `MockHost::revert_to`.
#[derive(Clone, Debug, Default)]
struct MockAccount {
    balance: [u8; 32],
    nonce: u64,
    code: Vec<u8>,
    storage: HashMap<[u8; 32], [u8; 32]>,
}

#[derive(Clone, Debug)]
struct MockSnapshot {
    id: SnapshotId,
    accounts: HashMap<[u8; 20], MockAccount>,
    created_this_tx: std::collections::HashSet<[u8; 20]>,
}

/// In-memory host for unit/integration tests. Snapshots are full deep copies
/// of the account map — fine for tests, never for production.
pub struct MockHost {
    accounts: HashMap<[u8; 20], MockAccount>,
    created_this_tx: std::collections::HashSet<[u8; 20]>,
    snapshots: Vec<MockSnapshot>,
    next_snapshot_id: SnapshotId,
    /// Ring buffer of recent block hashes, keyed by block number.
    /// Production nodes populate this from their block store; tests use
    /// `set_block_hash` to install known hashes for BLOCKHASH opcode tests.
    block_hashes: HashMap<u64, [u8; 32]>,
}

impl Default for MockHost {
    fn default() -> Self { Self::new() }
}

impl MockHost {
    pub fn new() -> Self {
        Self {
            accounts: HashMap::new(),
            created_this_tx: std::collections::HashSet::new(),
            snapshots: Vec::new(),
            next_snapshot_id: 1,
            block_hashes: HashMap::new(),
        }
    }

    /// Test helper — install a known block hash (for BLOCKHASH opcode tests).
    /// In production nodes this is populated from the chain's block store during
    /// block processing, keeping the most recent 256 entries.
    pub fn set_block_hash(&mut self, block_number: u64, hash: [u8; 32]) {
        self.block_hashes.insert(block_number, hash);
    }

    /// Test helper — install code at an address.
    pub fn install_code(&mut self, addr: &Address, code: Vec<u8>) {
        let entry = self.accounts.entry(*addr.as_bytes()).or_default();
        entry.code = code;
    }

    /// Test helper — credit balance directly.
    pub fn credit(&mut self, addr: &Address, value: [u8; 32]) {
        let entry = self.accounts.entry(*addr.as_bytes()).or_default();
        entry.balance = u256_add(&entry.balance, &value);
    }

    /// Test helper — set nonce.
    pub fn set_nonce(&mut self, addr: &Address, nonce: u64) {
        let entry = self.accounts.entry(*addr.as_bytes()).or_default();
        entry.nonce = nonce;
    }
}

impl Host for MockHost {
    fn balance(&self, addr: &Address) -> [u8; 32] {
        self.accounts.get(addr.as_bytes()).map(|a| a.balance).unwrap_or([0u8; 32])
    }
    fn nonce(&self, addr: &Address) -> u64 {
        self.accounts.get(addr.as_bytes()).map(|a| a.nonce).unwrap_or(0)
    }
    fn code(&self, addr: &Address) -> Vec<u8> {
        self.accounts.get(addr.as_bytes()).map(|a| a.code.clone()).unwrap_or_default()
    }
    fn code_hash(&self, addr: &Address) -> [u8; 32] {
        let code = self.code(addr);
        if code.is_empty() {
            return [0u8; 32];
        }
        zbx_crypto::keccak::keccak256(&code).0
    }
    fn storage_load(&self, addr: &Address, key: &[u8; 32]) -> [u8; 32] {
        self.accounts
            .get(addr.as_bytes())
            .and_then(|a| a.storage.get(key).copied())
            .unwrap_or([0u8; 32])
    }
    fn storage_store(&mut self, addr: &Address, key: [u8; 32], value: [u8; 32]) {
        let entry = self.accounts.entry(*addr.as_bytes()).or_default();
        entry.storage.insert(key, value);
    }
    fn set_code(&mut self, addr: &Address, code: Vec<u8>) {
        let entry = self.accounts.entry(*addr.as_bytes()).or_default();
        entry.code = code;
    }
    fn inc_nonce(&mut self, addr: &Address) -> Result<u64, EvmError> {
        let entry = self.accounts.entry(*addr.as_bytes()).or_default();
        entry.nonce = entry.nonce.checked_add(1)
            .ok_or(EvmError::NonceOverflow)?;
        Ok(entry.nonce)
    }
    fn transfer(&mut self, from: &Address, to: &Address, value: &[u8; 32])
        -> Result<(), EvmError>
    {
        if value == &[0u8; 32] { return Ok(()); }
        let from_bal = self.balance(from);
        if from_bal.as_slice() < value.as_slice() {
            return Err(EvmError::InsufficientBalance);
        }
        let new_from_bal = u256_sub(&from_bal, value);
        let to_bal = self.balance(to);
        let new_to_bal = u256_add(&to_bal, value);
        self.accounts.entry(*from.as_bytes()).or_default().balance = new_from_bal;
        self.accounts.entry(*to.as_bytes()).or_default().balance = new_to_bal;
        Ok(())
    }
    fn is_empty(&self, addr: &Address) -> bool {
        match self.accounts.get(addr.as_bytes()) {
            None => true,
            Some(a) => a.nonce == 0 && a.balance == [0u8; 32] && a.code.is_empty(),
        }
    }
    fn destruct(&mut self, addr: &Address, beneficiary: &Address) {
        // Always transfer balance.
        let bal = self.balance(addr);
        if bal != [0u8; 32] {
            // Self-destruct to self is a no-op transfer (balance stays).
            if addr.as_bytes() != beneficiary.as_bytes() {
                let _ = self.transfer(addr, beneficiary, &bal);
            }
        }
        // EIP-6780: only purge code+storage if created this tx.
        if self.was_created_this_tx(addr) {
            if let Some(a) = self.accounts.get_mut(addr.as_bytes()) {
                a.code.clear();
                a.storage.clear();
                a.nonce = 0;
                a.balance = [0u8; 32];
            }
        }
    }
    fn mark_created_this_tx(&mut self, addr: &Address) {
        self.created_this_tx.insert(*addr.as_bytes());
    }
    fn was_created_this_tx(&self, addr: &Address) -> bool {
        self.created_this_tx.contains(addr.as_bytes())
    }
    /// Task #3 (Precompile 0x0A — PayID resolution): real state-backed
    /// resolution against `MockHost`'s own storage. Mirrors what a
    /// production state-backed EVM host must do — read the registrar
    /// slot under [`PAYID_REGISTRAR_ADDR`] and unpack the right-aligned
    /// address. Returning `None` for the all-zero slot is observed by
    /// the precompile as `address(0)`, NOT as a revert.
    fn resolve_pay_id_bytes(&self, name: &[u8]) -> Option<[u8; 20]> {
        use zbx_types::payid::{payid_forward_slot, validate_payid_name, PAYID_REGISTRAR_ADDR};
        if !validate_payid_name(name) {
            return None;
        }
        let registrar = Address::from_bytes(&PAYID_REGISTRAR_ADDR).ok()?;
        let word = self.storage_load(&registrar, &payid_forward_slot(name));
        if word.iter().all(|&b| b == 0) {
            return None;
        }
        let mut out = [0u8; 20];
        out.copy_from_slice(&word[12..32]);
        Some(out)
    }

    /// Task #3 (Precompile 0x0A): real state-backed reverse resolution.
    /// Reads `keccak256("payid_rev/" || addr)` at [`PAYID_REGISTRAR_ADDR`]
    /// and returns the ASCII name (left-aligned, zero-padded). Returns
    /// `None` if the slot is the all-zero word.
    fn reverse_pay_id(&self, addr: &[u8; 20]) -> Option<Vec<u8>> {
        use zbx_types::payid::{payid_reverse_slot, PAYID_REGISTRAR_ADDR};
        let registrar = Address::from_bytes(&PAYID_REGISTRAR_ADDR).ok()?;
        let word = self.storage_load(&registrar, &payid_reverse_slot(addr));
        if word.iter().all(|&b| b == 0) {
            return None;
        }
        let len = word.iter().position(|&b| b == 0).unwrap_or(32);
        Some(word[..len].to_vec())
    }

    fn block_hash(&self, block_number: u64) -> [u8; 32] {
        self.block_hashes
            .get(&block_number)
            .copied()
            .unwrap_or([0u8; 32])
    }

    fn snapshot(&mut self) -> SnapshotId {
        let id = self.next_snapshot_id;
        self.next_snapshot_id = self.next_snapshot_id.wrapping_add(1);
        self.snapshots.push(MockSnapshot {
            id,
            accounts: self.accounts.clone(),
            created_this_tx: self.created_this_tx.clone(),
        });
        id
    }
    fn revert_to(&mut self, id: SnapshotId) {
        if let Some(pos) = self.snapshots.iter().rposition(|s| s.id == id) {
            // Drain everything strictly above and including pos; the deepest
            // snapshot found at `pos` carries the state we want to restore.
            let snap = self.snapshots.remove(pos);
            // Discard any nested snapshots that were taken after `id`.
            self.snapshots.truncate(pos);
            self.accounts = snap.accounts;
            self.created_this_tx = snap.created_this_tx;
        }
    }
    fn commit(&mut self, id: SnapshotId) {
        // Drop the named snapshot — its child writes stay folded in the
        // current state. Higher-numbered snapshots untouched.
        if let Some(pos) = self.snapshots.iter().rposition(|s| s.id == id) {
            self.snapshots.remove(pos);
        }
    }
}

// ---------------------------------------------------------------------------
//  Local big-endian U256 helpers (kept private to host.rs).
//  Stack already exposes equivalent fns but importing them from inside this
//  module would create a cycle; the math is trivial enough to inline.
// ---------------------------------------------------------------------------

fn u256_add(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    let mut out = [0u8; 32];
    let mut carry: u16 = 0;
    for i in (0..32).rev() {
        let s = a[i] as u16 + b[i] as u16 + carry;
        out[i] = s as u8;
        carry = s >> 8;
    }
    out
}

fn u256_sub(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    let mut out = [0u8; 32];
    let mut borrow: i16 = 0;
    for i in (0..32).rev() {
        let d = a[i] as i16 - b[i] as i16 - borrow;
        out[i] = d as u8;
        borrow = if d < 0 { 1 } else { 0 };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(b: u8) -> Address {
        let mut a = [0u8; 20];
        a[19] = b;
        Address(a)
    }

    fn val_u8(b: u8) -> [u8; 32] {
        let mut v = [0u8; 32];
        v[31] = b;
        v
    }

    #[test]
    fn snapshot_revert_restores_balances() {
        let mut h = MockHost::new();
        let a = addr(1);
        let b = addr(2);
        h.credit(&a, val_u8(100));
        let s = h.snapshot();
        h.transfer(&a, &b, &val_u8(40)).unwrap();
        assert_eq!(h.balance(&a), val_u8(60));
        assert_eq!(h.balance(&b), val_u8(40));
        h.revert_to(s);
        assert_eq!(h.balance(&a), val_u8(100));
        assert_eq!(h.balance(&b), val_u8(0));
    }

    #[test]
    fn nested_snapshots_revert_to_correct_layer() {
        let mut h = MockHost::new();
        let a = addr(1);
        h.credit(&a, val_u8(100));
        let s1 = h.snapshot();
        h.storage_store(&a, [1u8; 32], [9u8; 32]);
        let s2 = h.snapshot();
        h.storage_store(&a, [2u8; 32], [8u8; 32]);
        // revert_to s2 wipes s2's writes but preserves s1's.
        h.revert_to(s2);
        assert_eq!(h.storage_load(&a, &[1u8; 32]), [9u8; 32]);
        assert_eq!(h.storage_load(&a, &[2u8; 32]), [0u8; 32]);
        // revert_to s1 wipes s1's writes too.
        h.revert_to(s1);
        assert_eq!(h.storage_load(&a, &[1u8; 32]), [0u8; 32]);
    }

    #[test]
    fn commit_keeps_writes_visible_to_outer_frame() {
        let mut h = MockHost::new();
        let a = addr(1);
        let outer = h.snapshot();
        let inner = h.snapshot();
        h.storage_store(&a, [1u8; 32], [7u8; 32]);
        h.commit(inner);
        assert_eq!(h.storage_load(&a, &[1u8; 32]), [7u8; 32]);
        // Outer revert still rolls everything back.
        h.revert_to(outer);
        assert_eq!(h.storage_load(&a, &[1u8; 32]), [0u8; 32]);
    }

    #[test]
    fn transfer_insufficient_balance() {
        let mut h = MockHost::new();
        let a = addr(1);
        let b = addr(2);
        h.credit(&a, val_u8(10));
        let res = h.transfer(&a, &b, &val_u8(50));
        assert!(matches!(res, Err(EvmError::InsufficientBalance)));
        // Balances unchanged on failure.
        assert_eq!(h.balance(&a), val_u8(10));
        assert_eq!(h.balance(&b), val_u8(0));
    }

    #[test]
    fn is_empty_eip161() {
        let mut h = MockHost::new();
        let a = addr(1);
        assert!(h.is_empty(&a));
        h.credit(&a, val_u8(1));
        assert!(!h.is_empty(&a));
    }

    #[test]
    fn destruct_eip6780_only_purges_if_created_this_tx() {
        let mut h = MockHost::new();
        let a = addr(1);
        let b = addr(2);
        h.credit(&a, val_u8(50));
        h.install_code(&a, vec![0x60, 0x00]); // PUSH1 0
        // Not created this tx — code stays after destruct, balance moves.
        h.destruct(&a, &b);
        assert_eq!(h.balance(&a), val_u8(0));
        assert_eq!(h.balance(&b), val_u8(50));
        assert_eq!(h.code(&a), vec![0x60, 0x00], "code must persist (EIP-6780)");

        // Now mark created this tx and destruct again — code purges.
        h.credit(&a, val_u8(10));
        h.mark_created_this_tx(&a);
        h.destruct(&a, &b);
        assert_eq!(h.code(&a), vec![] as Vec<u8>);
    }

    #[test]
    fn nonce_overflow_errors() {
        let mut h = MockHost::new();
        let a = addr(1);
        h.set_nonce(&a, u64::MAX);
        let res = h.inc_nonce(&a);
        assert!(matches!(res, Err(EvmError::NonceOverflow)));
    }
}
