//! Production `ZvmHost` impl for the live chain state.
//!
//! Generic over a small [`ZvmStateAccess`] trait so the same host body
//! works for both `StateDB` (this crate, persistent / batched) and
//! `StateView` (zbx-execution, in-memory diff overlay used by the
//! block executor). The block-execution path uses the StateView impl;
//! tests and tooling that operate directly on `StateDB` use the
//! native impl.

use std::collections::{HashMap, HashSet};
use zbx_types::{
    account::{AccountState, VmKind},
    address::Address as ZbxAddress,
    block::BlockHeader,
    receipt::Log,
    H256,
};
use zbx_zvm::{context::Address as ZvmAddress, error::ZvmError, host::ZvmHost};

/// Per-tx EIP-1153 transient-storage scratchpad.
pub type TransientScratchpad = HashMap<(ZvmAddress, [u8; 32]), [u8; 32]>;

/// Minimum live-state surface the production ZVM host needs.
///
/// Implemented by both `StateDB` (in this crate) and `StateView` (in
/// zbx-execution). Methods are tight wrappers over the underlying
/// account / storage / code / log primitives.
pub trait ZvmStateAccess {
    fn get_account(&self, addr: &ZbxAddress) -> AccountState;
    fn set_account(&mut self, addr: ZbxAddress, state: AccountState);
    fn get_storage_word(&self, addr: &ZbxAddress, key: &[u8; 32]) -> [u8; 32];
    fn set_storage_word(&mut self, addr: ZbxAddress, key: [u8; 32], value: [u8; 32]);
    /// Resolve a contract's runtime bytecode by address.
    fn get_code_for(&self, addr: &ZbxAddress) -> Vec<u8>;
    /// Persist runtime bytecode keyed by its keccak256 hash.
    fn seed_code(&mut self, code_hash: H256, code: Vec<u8>);
    /// Append an EVM-compatible log to the receipt log tail.
    fn emit_log(&mut self, log: Log);
}

/// Header-derived fields the ZVM host exposes (COINBASE / GASLIMIT /
/// PREVRANDAO / GASPRICE / BLOBHASH).
#[derive(Clone, Debug)]
pub struct ZvmBlockEnv {
    pub coinbase:        ZvmAddress,
    pub block_gas_limit: u64,
    pub prevrandao:      [u8; 32],
    pub gas_price:       u128,
    pub blob_hashes:     Vec<[u8; 32]>,
}

impl ZvmBlockEnv {
    /// Snapshot the host-visible fields from a `BlockHeader`.
    /// `prevrandao` reads `header.mix_hash` (the post-merge PREVRANDAO
    /// carrier on this chain). `blob_hashes` is empty — the chain
    /// header doesn't yet carry an EIP-4844 blob sidecar; once it does
    /// the producer should call [`ZvmBlockEnv::from_header_with_blob_hashes`]
    /// instead.
    pub fn from_header(header: &BlockHeader) -> Self {
        Self::from_header_with_blob_hashes(header, Vec::new())
    }

    /// Same as [`from_header`] but with caller-provided versioned
    /// blob hashes (EIP-4844). The hashes are passed straight through
    /// to the BLOBHASH opcode; index out of range returns the zero
    /// word per spec.
    pub fn from_header_with_blob_hashes(
        header: &BlockHeader,
        blob_hashes: Vec<[u8; 32]>,
    ) -> Self {
        ZvmBlockEnv {
            coinbase:        header.coinbase.0,
            block_gas_limit: header.gas_limit,
            prevrandao:      header.mix_hash.0,
            gas_price:       header.base_fee_per_gas as u128,
            blob_hashes,
        }
    }
}

/// Production ZVM host — `&mut S: ZvmStateAccess` + cached block env
/// + per-tx transient scratchpad.
///
/// `caller_vm` records which VM kind the calling frame runs under,
/// so cross-VM CALLs are rejected at the [`ZvmHost::is_call_allowed`]
/// gate (a ZVM frame may not call into an EVM-deployed account, and
/// vice-versa). The interpreter pushes 0 on rejection without
/// executing any sub-bytecode.
pub struct ProductionZvmHost<'a, S: ZvmStateAccess> {
    pub state:     &'a mut S,
    pub env:       &'a ZvmBlockEnv,
    pub transient: &'a mut TransientScratchpad,
    pub caller_vm: VmKind,
    /// Task #8 (EIP-6780): per-tx CREATE/CREATE2 set. The host owns
    /// it (single instance shared across every sub-frame because the
    /// interpreter passes `&mut *self.host` into sub-frames). The
    /// executor reads it after the top-level call returns to gate
    /// full deletion vs sweep-only.
    pub created_this_tx:  HashSet<ZvmAddress>,
    /// Task #8 (EIP-6780): pending SELFDESTRUCT (contract, beneficiary)
    /// pairs accumulated this tx. The executor drains via
    /// [`Self::take_pending_destructs`] after the top-level call
    /// returns and applies full deletion only to entries whose
    /// `contract` is in [`Self::created_this_tx`].
    pub pending_destructs: Vec<(ZvmAddress, ZvmAddress)>,
}

impl<'a, S: ZvmStateAccess> ProductionZvmHost<'a, S> {
    pub fn new(
        state: &'a mut S,
        env: &'a ZvmBlockEnv,
        transient: &'a mut TransientScratchpad,
    ) -> Self {
        ProductionZvmHost {
            state,
            env,
            transient,
            caller_vm: VmKind::Zvm,
            created_this_tx:  HashSet::new(),
            pending_destructs: Vec::new(),
        }
    }

    /// Drain the pending-destruct queue. The executor calls this at
    /// end-of-tx (only on success) and applies full account deletion
    /// to entries whose `contract` is in `created_this_tx`.
    pub fn take_pending_destructs(&mut self) -> Vec<(ZvmAddress, ZvmAddress)> {
        std::mem::take(&mut self.pending_destructs)
    }
}

/// Wipe the EIP-1153 transient scratchpad at end-of-tx. Idempotent.
pub fn clear_transient(scratchpad: &mut TransientScratchpad) {
    scratchpad.clear();
}

#[inline]
fn to_zbx(addr: &ZvmAddress) -> ZbxAddress {
    ZbxAddress(*addr)
}

impl<'a, S: ZvmStateAccess> ZvmHost for ProductionZvmHost<'a, S> {
    fn balance(&self, addr: &ZvmAddress) -> u128 {
        self.state.get_account(&to_zbx(addr)).balance_u128()
    }

    fn storage_load(&self, addr: &ZvmAddress, key: &[u8; 32]) -> [u8; 32] {
        self.state.get_storage_word(&to_zbx(addr), key)
    }

    fn storage_store(&mut self, addr: &ZvmAddress, key: [u8; 32], value: [u8; 32]) {
        self.state.set_storage_word(to_zbx(addr), key, value);
    }

    fn code(&self, addr: &ZvmAddress) -> Vec<u8> {
        self.state.get_code_for(&to_zbx(addr))
    }

    fn code_hash(&self, addr: &ZvmAddress) -> [u8; 32] {
        self.state.get_account(&to_zbx(addr)).code_hash.0
    }

    fn block_hash(&self, _block: u64) -> [u8; 32] {
        [0u8; 32]
    }

    fn transfer(&mut self, from: &ZvmAddress, to: &ZvmAddress, amount: u128) -> Result<(), ZvmError> {
        let f = to_zbx(from);
        let t = to_zbx(to);
        let mut from_acct = self.state.get_account(&f);
        let from_bal = from_acct.balance_u128();
        if from_bal < amount {
            return Err(ZvmError::InsufficientBalance);
        }
        from_acct.set_balance_u128(from_bal - amount);
        self.state.set_account(f, from_acct);

        let mut to_acct = self.state.get_account(&t);
        let to_bal = to_acct.balance_u128();
        to_acct.set_balance_u128(to_bal.saturating_add(amount));
        self.state.set_account(t, to_acct);
        Ok(())
    }

    fn nonce(&self, addr: &ZvmAddress) -> u64 {
        self.state.get_account(&to_zbx(addr)).nonce
    }

    fn inc_nonce(&mut self, addr: &ZvmAddress) -> u64 {
        let a = to_zbx(addr);
        let mut acct = self.state.get_account(&a);
        acct.nonce = acct.nonce.saturating_add(1);
        let n = acct.nonce;
        self.state.set_account(a, acct);
        n
    }

    fn set_code(&mut self, addr: &ZvmAddress, code: Vec<u8>) {
        // Sub-frame CREATE: persist runtime bytecode + code_hash and
        // mark the new account as ZVM-deployed (this frame is ZVM).
        use zbx_crypto::keccak::keccak256;
        let a = to_zbx(addr);
        let code_hash = keccak256(&code);
        self.state.seed_code(code_hash, code);
        let mut acct = self.state.get_account(&a);
        acct.code_hash = code_hash;
        acct.vm = VmKind::Zvm;
        self.state.set_account(a, acct);
    }

    fn resolve_pay_id(&self, pay_id: &str) -> Option<ZvmAddress> {
        // Strip optional "@zbx" suffix, then forward to the byte-based
        // resolver that reads the chain's PayID registrar storage.
        let lowered = pay_id.to_ascii_lowercase();
        let name = lowered.trim_end_matches("@zbx");
        self.resolve_pay_id_bytes(name.as_bytes())
    }

    /// Task #3 (Precompile 0x0A): real on-chain forward resolution. Reads
    /// `keccak256("payid/" || name)` at [`PAYID_REGISTRAR_ADDR`] and
    /// returns the address right-aligned in the 32-byte word, or `None`
    /// if the slot is the all-zero word.
    fn resolve_pay_id_bytes(&self, name: &[u8]) -> Option<ZvmAddress> {
        use zbx_types::payid::{payid_forward_slot, validate_payid_name, PAYID_REGISTRAR_ADDR};
        if !validate_payid_name(name) {
            return None;
        }
        let slot = payid_forward_slot(name);
        let registrar = ZbxAddress(PAYID_REGISTRAR_ADDR);
        let word = self.state.get_storage_word(&registrar, &slot);
        if word.iter().all(|&b| b == 0) {
            return None;
        }
        let mut out = [0u8; 20];
        out.copy_from_slice(&word[12..32]);
        Some(out)
    }

    /// Task #3 (Precompile 0x0A): real on-chain reverse resolution. Reads
    /// `keccak256("payid_rev/" || addr)` at [`PAYID_REGISTRAR_ADDR`] and
    /// extracts the ASCII name (left-aligned, zero-padded). Returns
    /// `None` if the slot is the all-zero word.
    fn reverse_pay_id(&self, addr: &ZvmAddress) -> Option<Vec<u8>> {
        use zbx_types::payid::{payid_reverse_slot, PAYID_REGISTRAR_ADDR};
        let slot = payid_reverse_slot(addr);
        let registrar = ZbxAddress(PAYID_REGISTRAR_ADDR);
        let word = self.state.get_storage_word(&registrar, &slot);
        if word.iter().all(|&b| b == 0) {
            return None;
        }
        // Name is left-aligned, zero-padded to 32 bytes.
        let len = word.iter().position(|&b| b == 0).unwrap_or(32);
        Some(word[..len].to_vec())
    }
    fn zusd_balance(&self, _addr: &ZvmAddress) -> u128 { 0 }
    fn zbx_price_usd(&self) -> u128 { 0 }
    fn blob_base_fee(&self) -> u128 { 1 }

    fn burn_zbx(&mut self, addr: &ZvmAddress, amount: u128) -> Result<(), ZvmError> {
        let a = to_zbx(addr);
        let mut acct = self.state.get_account(&a);
        let bal = acct.balance_u128();
        if bal < amount {
            return Err(ZvmError::InsufficientBalance);
        }
        acct.set_balance_u128(bal - amount);
        self.state.set_account(a, acct);
        Ok(())
    }

    fn emit_zvm_log(&mut self, _key: &str, _value: &str) {}

    fn transient_load(&self, addr: &ZvmAddress, key: &[u8; 32]) -> [u8; 32] {
        self.transient.get(&(*addr, *key)).copied().unwrap_or([0u8; 32])
    }

    fn transient_store(&mut self, addr: &ZvmAddress, key: [u8; 32], value: [u8; 32]) {
        self.transient.insert((*addr, key), value);
    }

    fn coinbase(&self) -> ZvmAddress { self.env.coinbase }
    fn block_gas_limit(&self) -> u64 { self.env.block_gas_limit }
    fn prevrandao(&self) -> [u8; 32] { self.env.prevrandao }
    fn gas_price(&self) -> u128 { self.env.gas_price }
    fn blob_hash(&self, i: u64) -> [u8; 32] {
        self.env.blob_hashes.get(i as usize).copied().unwrap_or([0u8; 32])
    }

    /// Cross-VM CALL gate. A contract account whose `vm` differs from
    /// the calling frame's `caller_vm` is rejected; EOAs and
    /// pre-deploy stubs (no code) pass through (EVM and ZVM share the
    /// same plain-value-transfer semantics for those).
    fn is_call_allowed(&self, target: &ZvmAddress) -> bool {
        let acct = self.state.get_account(&to_zbx(target));
        if !acct.is_contract() {
            return true;
        }
        acct.vm == self.caller_vm
    }

    // ── Task #8 (EIP-6780) ───────────────────────────────────────────────
    fn mark_created_this_tx(&mut self, addr: &ZvmAddress) {
        self.created_this_tx.insert(*addr);
    }
    fn was_created_this_tx(&self, addr: &ZvmAddress) -> bool {
        self.created_this_tx.contains(addr)
    }
    fn selfdestruct(&mut self, contract: &ZvmAddress, beneficiary: &ZvmAddress) {
        // Sweep balance unconditionally (EIP-6780 keeps this leg
        // regardless of whether the contract was created this tx).
        let bal = self.balance(contract);
        if bal > 0 {
            let _ = self.transfer(contract, beneficiary, bal);
        }
        // Enqueue for the executor's end-of-tx drain — full deletion
        // is gated on `was_created_this_tx(contract)` there.
        self.pending_destructs.push((*contract, *beneficiary));
    }

    fn emit_log(&mut self, addr: &ZvmAddress, topics: Vec<[u8; 32]>, data: Vec<u8>) {
        self.state.emit_log(Log {
            address: to_zbx(addr),
            topics: topics.into_iter().map(H256).collect(),
            data,
            block_number: 0,
            log_index: 0,
            transaction_hash: H256::zero(),
            transaction_index: 0,
        });
    }
}

// ─── Native StateDB impl of ZvmStateAccess ───────────────────────────────

impl ZvmStateAccess for crate::state_db::StateDB {
    fn get_account(&self, addr: &ZbxAddress) -> AccountState {
        self.get_account(addr)
    }
    fn set_account(&mut self, addr: ZbxAddress, state: AccountState) {
        self.set_account(addr, state);
    }
    fn get_storage_word(&self, addr: &ZbxAddress, key: &[u8; 32]) -> [u8; 32] {
        self.get_storage(addr, &H256(*key)).0
    }
    fn set_storage_word(&mut self, addr: ZbxAddress, key: [u8; 32], value: [u8; 32]) {
        self.set_storage(addr, H256(key), H256(value));
    }
    fn get_code_for(&self, addr: &ZbxAddress) -> Vec<u8> {
        let acct = self.get_account(addr);
        self.get_code(&acct.code_hash)
    }
    fn seed_code(&mut self, code_hash: H256, code: Vec<u8>) {
        self.seed_code(code_hash, code);
    }
    fn emit_log(&mut self, log: Log) {
        self.emit_log(log);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbx_types::U256;

    fn header_with_mix(mix: H256) -> BlockHeader {
        BlockHeader {
            parent_hash: H256::zero(),
            uncle_hash: H256::zero(),
            coinbase: ZbxAddress([0u8; 20]),
            state_root: H256::zero(),
            transactions_root: H256::zero(),
            receipts_root: H256::zero(),
            logs_bloom: [0u8; 256],
            difficulty: U256::zero(),
            number: 1,
            gas_limit: 30_000_000,
            gas_used: 0,
            timestamp: 0,
            extra_data: Vec::new(),
            mix_hash: mix,
            nonce: 0,
            base_fee_per_gas: 7,
            committee_signature: Vec::new(),
            epoch: 0,
            epoch_seed: None,
        }
    }

    #[test]
    fn from_header_reads_mix_hash_as_prevrandao() {
        let mut mix = [0u8; 32];
        mix[0] = 0xDE;
        mix[31] = 0xAD;
        let h = header_with_mix(H256(mix));
        let env = ZvmBlockEnv::from_header(&h);
        assert_eq!(env.prevrandao, mix);
        assert_eq!(env.gas_price, 7);
    }

    /// Minimal in-memory `ZvmStateAccess` for unit-testing the cross-VM
    /// CALL gate without dragging in `StateDB` / RocksDB.
    #[derive(Default)]
    struct MemState {
        accts: HashMap<ZbxAddress, AccountState>,
    }
    impl ZvmStateAccess for MemState {
        fn get_account(&self, a: &ZbxAddress) -> AccountState {
            self.accts.get(a).cloned().unwrap_or_default()
        }
        fn set_account(&mut self, a: ZbxAddress, s: AccountState) { self.accts.insert(a, s); }
        fn get_storage_word(&self, _: &ZbxAddress, _: &[u8;32]) -> [u8;32] { [0u8;32] }
        fn set_storage_word(&mut self, _: ZbxAddress, _: [u8;32], _: [u8;32]) {}
        fn get_code_for(&self, _: &ZbxAddress) -> Vec<u8> { Vec::new() }
        fn seed_code(&mut self, _: H256, _: Vec<u8>) {}
        fn emit_log(&mut self, _: Log) {}
    }

    fn contract(vm: VmKind) -> AccountState {
        let mut a = AccountState::default();
        // Non-empty code_hash so `is_contract()` returns true.
        a.code_hash = H256([0xCC; 32]);
        a.vm = vm;
        a
    }

    #[test]
    fn is_call_allowed_rejects_cross_vm_and_permits_same_vm_and_eoa() {
        let mut state = MemState::default();
        let zvm_target = ZbxAddress([0x11; 20]);
        let evm_target = ZbxAddress([0x22; 20]);
        let eoa_target = ZbxAddress([0x33; 20]);
        state.set_account(zvm_target, contract(VmKind::Zvm));
        state.set_account(evm_target, contract(VmKind::Evm));
        // EOA: no code_hash override → is_contract() == false.

        let env = ZvmBlockEnv::from_header(&header_with_mix(H256::zero()));
        let mut transient = TransientScratchpad::new();
        let host = ProductionZvmHost::new(&mut state, &env, &mut transient);
        // Default caller_vm = Zvm.
        assert!(host.is_call_allowed(&zvm_target.0), "ZVM→ZVM allowed");
        assert!(!host.is_call_allowed(&evm_target.0), "ZVM→EVM rejected");
        assert!(host.is_call_allowed(&eoa_target.0), "ZVM→EOA allowed (plain transfer)");
    }

    #[test]
    fn from_header_with_blob_hashes_passthrough() {
        let h = header_with_mix(H256::zero());
        let mut b0 = [0u8; 32]; b0[0] = 0x01;
        let mut b1 = [0u8; 32]; b1[31] = 0x02;
        let env = ZvmBlockEnv::from_header_with_blob_hashes(&h, vec![b0, b1]);
        assert_eq!(env.blob_hashes.len(), 2);
        assert_eq!(env.blob_hashes[0], b0);
        assert_eq!(env.blob_hashes[1], b1);
    }
}
