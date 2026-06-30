//! ZVM host interface — provides state access to the interpreter.
//!
//! The host bridges the ZVM interpreter and the chain state.
//! It handles: storage reads/writes, balance queries, code access,
//! Pay ID resolution, ZUSD balance reads, ZBX price oracle, and
//! (C53-02) sub-call state mutation (transfer, nonce, set_code).

use crate::{context::Address, error::ZvmError};

/// ZVM host interface. Implemented by the chain state backend.
pub trait ZvmHost {
    // ── EVM-compatible ────────────────────────────────────────────────────

    /// Get ZBX balance of an address (in wei).
    fn balance(&self, addr: &Address) -> u128;

    /// Get storage value at (address, key).
    fn storage_load(&self, addr: &Address, key: &[u8; 32]) -> [u8; 32];

    /// Set storage value at (address, key).
    fn storage_store(&mut self, addr: &Address, key: [u8; 32], value: [u8; 32]);

    /// Get bytecode of a contract.
    fn code(&self, addr: &Address) -> Vec<u8>;

    /// Get code hash of a contract.
    fn code_hash(&self, addr: &Address) -> [u8; 32];

    /// Get code size of a contract.
    fn code_size(&self, addr: &Address) -> usize {
        self.code(addr).len()
    }

    /// Get a historical block hash by block number.
    fn block_hash(&self, block: u64) -> [u8; 32];

    // ── Sub-call state mutation (C53-02) ──────────────────────────────────

    /// Transfer `amount` wei from `from` to `to`.
    /// Default no-op — production host overrides with real balance mutation.
    fn transfer(&mut self, from: &Address, to: &Address, amount: u128) -> Result<(), ZvmError> {
        let _ = (from, to, amount);
        Ok(())
    }

    /// Get the transaction nonce of an address.
    /// Default 0 — production host overrides for correct CREATE addressing.
    fn nonce(&self, addr: &Address) -> u64 {
        let _ = addr;
        0
    }

    /// Increment and return the new nonce of an address.
    /// Default no-op — production host overrides.
    fn inc_nonce(&mut self, addr: &Address) -> u64 {
        let _ = addr;
        1
    }

    /// Deploy bytecode to `addr`.
    /// Default no-op — production host overrides.
    fn set_code(&mut self, addr: &Address, code: Vec<u8>) {
        let _ = (addr, code);
    }

    /// Check if an account is empty (no balance, no code, nonce = 0).
    fn is_empty(&self, addr: &Address) -> bool {
        self.balance(addr) == 0
            && self.code_size(addr) == 0
            && self.nonce(addr) == 0
    }

    // ── ZVM-native ────────────────────────────────────────────────────────

    /// Resolve a Pay ID string to an address.
    fn resolve_pay_id(&self, pay_id: &str) -> Option<Address>;

    /// Task #3 (Precompile 0x0A): byte-input forward resolver used by the
    /// 0x0A precompile dispatcher. The precompile feeds in raw ASCII
    /// (already validated against `[a-z0-9._-]{3,32}`); the host returns
    /// the registered address or `None` for unregistered names.
    /// Default delegates to [`resolve_pay_id`] so existing hosts that only
    /// implemented the string variant keep working.
    fn resolve_pay_id_bytes(&self, name: &[u8]) -> Option<Address> {
        let s = std::str::from_utf8(name).ok()?;
        self.resolve_pay_id(s)
    }

    /// Task #3 (Precompile 0x0A): reverse `address → name` lookup used by
    /// the 0x0A precompile when `op = 1`. Returns the ASCII name (no
    /// `@zbx` suffix) registered against `addr`, or `None` if no PayID
    /// has claimed the address. Default `None` keeps mock hosts working
    /// unchanged.
    fn reverse_pay_id(&self, addr: &Address) -> Option<Vec<u8>> {
        let _ = addr;
        None
    }

    /// Get ZUSD balance of an address (in 18-decimal wei).
    fn zusd_balance(&self, addr: &Address) -> u128;

    /// Get current ZBX/USD price from oracle (18 decimals).
    fn zbx_price_usd(&self) -> u128;

    /// Get current blob base fee (wei per byte).
    fn blob_base_fee(&self) -> u128;

    /// Check if an address has a registered Pay ID.
    fn has_pay_id(&self, addr: &Address) -> bool {
        let _ = addr;
        false
    }

    /// Burn ZBX from caller (deflationary mechanism).
    fn burn_zbx(&mut self, addr: &Address, amount: u128) -> Result<(), ZvmError>;

    /// Emit a ZVM structured log (key-value pair).
    fn emit_zvm_log(&mut self, key: &str, value: &str);

    // ── EIP-1153 Transient Storage (Cancun) ──────────────────────────────
    //
    // SEC-2026-05-09 Pass-16: TLOAD/TSTORE were missing entirely (fell
    // through to InvalidOpcode), bricking every Cancun-era reentrancy
    // guard pattern (OZ TransientReentrancyGuard, UniV4 Locker). Defaults
    // are no-op (read returns 0, write discarded) so existing test hosts
    // keep compiling; production host overrides with per-tx scratchpad
    // that is cleared at the end of every transaction.
    fn transient_load(&self, addr: &Address, key: &[u8; 32]) -> [u8; 32] {
        let _ = (addr, key);
        [0u8; 32]
    }
    fn transient_store(&mut self, addr: &Address, key: [u8; 32], value: [u8; 32]) {
        let _ = (addr, key, value);
    }

    /// Coinbase (block author) address — used by COINBASE opcode.
    fn coinbase(&self) -> Address { [0u8; 20] }

    /// Block gas limit — used by GASLIMIT opcode.
    fn block_gas_limit(&self) -> u64 { 30_000_000 }

    /// PREVRANDAO (EIP-4399) — beacon-chain mixed randomness for current block.
    /// Default returns block_hash(parent) for deterministic testing.
    fn prevrandao(&self) -> [u8; 32] { [0u8; 32] }

    /// Effective gas price for the executing transaction (EIP-1559: base+tip).
    fn gas_price(&self) -> u128 { 0 }

    /// EIP-4844 versioned blob hash at index `i`.
    fn blob_hash(&self, i: u64) -> [u8; 32] { let _ = i; [0u8; 32] }

    /// Pre-call gate. Returning `false` causes the interpreter's CALL
    /// family to skip sub-execution entirely and push `0` (failure)
    /// onto the stack — distinct from "called empty bytecode" which
    /// would push `1` (Success). Production hosts override this to
    /// reject cross-VM CALLs (a ZVM frame calling into an EVM-deployed
    /// account, or vice-versa). Default permits everything so existing
    /// test hosts keep their current semantics.
    fn is_call_allowed(&self, target: &Address) -> bool {
        let _ = target;
        true
    }

    // ── EIP-6780 SELFDESTRUCT + CreatedInTx (Task #8) ────────────────────
    //
    // Cancun EIP-6780 redefines SELFDESTRUCT: the account is FULLY
    // deleted (code + storage + tombstone) only if it was CREATEd in
    // the same transaction. Otherwise SELFDESTRUCT just sweeps the
    // balance to the beneficiary and the contract remains live.
    //
    // The interpreter calls `mark_created_this_tx(addr)` from CREATE /
    // CREATE2 success paths, and `selfdestruct(contract, beneficiary)`
    // from the SELFDESTRUCT opcode. The host owns the per-tx creation
    // set + the pending-destruct queue; the executor drains the queue
    // at end-of-tx and applies full deletion only for addresses in the
    // creation set.
    //
    // Defaults:
    //   - `mark_created_this_tx` / `was_created_this_tx`: no-op / false
    //     so test hosts that don't care about EIP-6780 keep working.
    //   - `selfdestruct`: balance sweep only (matches pre-Pass-15
    //     behaviour). Production host overrides to also enqueue the
    //     pending destruct so the executor can purge the account at
    //     end-of-tx if it was created this tx.

    /// Record that `addr` was deployed by CREATE / CREATE2 in the
    /// currently-executing transaction. Drives the EIP-6780 "delete
    /// only same-tx creates" gate at end-of-tx.
    fn mark_created_this_tx(&mut self, addr: &Address) {
        let _ = addr;
    }

    /// Was `addr` deployed by CREATE / CREATE2 in the currently-executing
    /// transaction? Used by SELFDESTRUCT bookkeeping and by the
    /// end-of-tx drain in the executor.
    fn was_created_this_tx(&self, addr: &Address) -> bool {
        let _ = addr;
        false
    }

    /// Execute SELFDESTRUCT semantics for `contract` (sweeping its
    /// balance to `beneficiary`). Default sweeps balance only — the
    /// contract code, storage, and account remain live (this matches
    /// pre-Pass-15 behaviour, which is the EIP-6780 outcome for any
    /// account NOT created in the current tx). Production hosts
    /// override to also enqueue the (contract, beneficiary) pair so
    /// the executor can apply full deletion at end-of-tx if
    /// `was_created_this_tx(contract)` is true.
    fn selfdestruct(&mut self, contract: &Address, beneficiary: &Address) {
        let bal = self.balance(contract);
        if bal > 0 {
            let _ = self.transfer(contract, beneficiary, bal);
        }
    }

    /// Emit an EVM-compatible log (LOG0–LOG4).
    ///
    /// SEC-2026-05-09 Pass-10: previously LOG0–LOG4 fell through to the
    /// `_` catch-all in the interpreter and aborted the frame as
    /// `InvalidOpcode`, breaking every ERC-20 / ERC-721 emit and bricking
    /// indexers. The interpreter now decodes the data + topics and forwards
    /// them here. Default implementation is a no-op so existing test hosts
    /// keep working; production hosts should override to populate receipts.
    fn emit_log(&mut self, addr: &Address, topics: Vec<[u8; 32]>, data: Vec<u8>) {
        let _ = (addr, topics, data);
    }
}

/// Mock host for testing — in-memory state.
///
/// SEC-2026-05-09 Pass-18: now ships with a real EIP-1153 transient-storage
/// scratchpad. The scratchpad is keyed `(contract_addr, slot)` and is
/// expected to be cleared by calling `clear_transient()` at the end of every
/// transaction (mirrors what the production host in `zbx-state` does after
/// `commit_block`). Tests that span multiple top-level calls within a single
/// "tx" naturally see the same scratchpad — matching Cancun semantics.
pub struct MockZvmHost {
    pub balances:  std::collections::HashMap<[u8; 20], u128>,
    pub storage:   std::collections::HashMap<([u8; 20], [u8; 32]), [u8; 32]>,
    pub code:      std::collections::HashMap<[u8; 20], Vec<u8>>,
    pub nonces:    std::collections::HashMap<[u8; 20], u64>,
    pub pay_ids:   std::collections::HashMap<String, [u8; 20]>,
    /// Task #3 (Precompile 0x0A): reverse `address → name` overrides for
    /// `MockZvmHost::reverse_pay_id`. Tests that exercise the 0x0A `op=1`
    /// path populate this directly.
    pub pay_ids_reverse: std::collections::HashMap<[u8; 20], String>,
    pub zbx_price: u128,
    pub blob_fee:  u128,
    /// Pass-18: per-transaction transient storage (TLOAD/TSTORE, EIP-1153).
    pub transient: std::collections::HashMap<([u8; 20], [u8; 32]), [u8; 32]>,
    /// Pass-18: header-derived fields exposed via the `coinbase` /
    /// `block_gas_limit` / `prevrandao` / `gas_price` / `blob_hash` host
    /// methods. Pre-Pass-18 these always returned zero defaults, which made
    /// every COINBASE / PREVRANDAO / GASLIMIT / GASPRICE / BLOBHASH opcode
    /// return zero — bricking every Cancun-era contract that branched on
    /// any of them. Production host (`zbx-state`) populates the same fields
    /// from `BlockHeader` at `BlockExecutor::execute_block` boundaries.
    pub coinbase:        [u8; 20],
    pub block_gas_limit: u64,
    pub prevrandao:      [u8; 32],
    pub gas_price:       u128,
    pub blob_hashes:     Vec<[u8; 32]>,
    /// Task #8 (EIP-6780): per-tx CREATE/CREATE2 set. Drives the
    /// "fully delete only same-tx creates" gate at end-of-tx.
    pub created_this_tx:  std::collections::HashSet<[u8; 20]>,
    /// Task #8 (EIP-6780): pending SELFDESTRUCT (contract, beneficiary)
    /// pairs accumulated this tx. Tests / executors drain via
    /// `take_pending_destructs()` at end-of-tx.
    pub pending_destructs: Vec<([u8; 20], [u8; 20])>,
}

impl MockZvmHost {
    pub fn new() -> Self {
        MockZvmHost {
            balances:        std::collections::HashMap::new(),
            storage:         std::collections::HashMap::new(),
            code:            std::collections::HashMap::new(),
            nonces:          std::collections::HashMap::new(),
            pay_ids:         std::collections::HashMap::new(),
            pay_ids_reverse: std::collections::HashMap::new(),
            zbx_price:       2_500 * 10u128.pow(18),
            blob_fee:        1,
            transient:       std::collections::HashMap::new(),
            coinbase:        [0u8; 20],
            block_gas_limit: 30_000_000,
            prevrandao:      [0u8; 32],
            gas_price:       0,
            blob_hashes:     Vec::new(),
            created_this_tx:  std::collections::HashSet::new(),
            pending_destructs: Vec::new(),
        }
    }

    /// Clear the EIP-1153 transient-storage scratchpad. The production host
    /// must call this at the end of every transaction; tests that simulate
    /// multiple txs against the same `MockZvmHost` should call it manually.
    pub fn clear_transient(&mut self) {
        self.transient.clear();
    }

    /// Task #8 (EIP-6780): drain the pending-destruct queue and reset
    /// the per-tx CREATE/CREATE2 set. Tests use this to inspect what
    /// SELFDESTRUCTs were enqueued during a tx; the executor uses the
    /// equivalent on `ProductionZvmHost`.
    pub fn take_pending_destructs(&mut self) -> Vec<([u8; 20], [u8; 20])> {
        std::mem::take(&mut self.pending_destructs)
    }

    /// Task #8 (EIP-6780): reset per-tx EIP-6780 state. Call after
    /// draining the pending-destruct queue so a subsequent tx against
    /// the same host starts with a clean creation set.
    pub fn clear_tx_state(&mut self) {
        self.created_this_tx.clear();
        self.pending_destructs.clear();
    }
}

impl ZvmHost for MockZvmHost {
    fn balance(&self, addr: &Address) -> u128 {
        self.balances.get(addr).copied().unwrap_or(0)
    }
    fn storage_load(&self, addr: &Address, key: &[u8; 32]) -> [u8; 32] {
        self.storage.get(&(*addr, *key)).copied().unwrap_or([0u8; 32])
    }
    fn storage_store(&mut self, addr: &Address, key: [u8; 32], value: [u8; 32]) {
        self.storage.insert((*addr, key), value);
    }
    fn code(&self, addr: &Address) -> Vec<u8> {
        self.code.get(addr).cloned().unwrap_or_default()
    }
    fn code_hash(&self, addr: &Address) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        let code = self.code(addr);
        let mut h = [0u8; 32];
        h.copy_from_slice(&Sha256::digest(&code));
        h
    }
    fn block_hash(&self, _block: u64) -> [u8; 32] { [0u8; 32] }

    fn transfer(&mut self, from: &Address, to: &Address, amount: u128) -> Result<(), ZvmError> {
        let from_bal = self.balances.get(from).copied().unwrap_or(0);
        if from_bal < amount {
            return Err(ZvmError::InsufficientBalance);
        }
        *self.balances.entry(*from).or_insert(0) -= amount;
        *self.balances.entry(*to).or_insert(0) += amount;
        Ok(())
    }
    fn nonce(&self, addr: &Address) -> u64 {
        self.nonces.get(addr).copied().unwrap_or(0)
    }
    fn inc_nonce(&mut self, addr: &Address) -> u64 {
        let n = self.nonces.entry(*addr).or_insert(0);
        *n += 1;
        *n
    }
    fn set_code(&mut self, addr: &Address, code: Vec<u8>) {
        self.code.insert(*addr, code);
    }

    fn resolve_pay_id(&self, pay_id: &str) -> Option<Address> {
        let key = pay_id.trim_end_matches("@zbx").to_lowercase();
        self.pay_ids.get(&key).copied()
    }
    fn reverse_pay_id(&self, addr: &Address) -> Option<Vec<u8>> {
        self.pay_ids_reverse.get(addr).map(|s| s.as_bytes().to_vec())
    }
    fn zusd_balance(&self, _addr: &Address) -> u128 { 0 }
    fn zbx_price_usd(&self) -> u128 { self.zbx_price }
    fn blob_base_fee(&self) -> u128 { self.blob_fee }
    fn burn_zbx(&mut self, addr: &Address, amount: u128) -> Result<(), ZvmError> {
        let bal = self.balances.entry(*addr).or_insert(0);
        if *bal < amount { return Err(ZvmError::InsufficientBalance); }
        *bal -= amount;
        Ok(())
    }
    fn emit_zvm_log(&mut self, _key: &str, _value: &str) {}

    // ── Pass-18: real transient storage + header fields ──────────────────

    fn transient_load(&self, addr: &Address, key: &[u8; 32]) -> [u8; 32] {
        self.transient.get(&(*addr, *key)).copied().unwrap_or([0u8; 32])
    }
    fn transient_store(&mut self, addr: &Address, key: [u8; 32], value: [u8; 32]) {
        self.transient.insert((*addr, key), value);
    }
    fn coinbase(&self) -> Address { self.coinbase }
    fn block_gas_limit(&self) -> u64 { self.block_gas_limit }
    fn prevrandao(&self) -> [u8; 32] { self.prevrandao }
    fn gas_price(&self) -> u128 { self.gas_price }
    fn blob_hash(&self, i: u64) -> [u8; 32] {
        self.blob_hashes.get(i as usize).copied().unwrap_or([0u8; 32])
    }

    // ── Task #8 (EIP-6780) ───────────────────────────────────────────────
    fn mark_created_this_tx(&mut self, addr: &Address) {
        self.created_this_tx.insert(*addr);
    }
    fn was_created_this_tx(&self, addr: &Address) -> bool {
        self.created_this_tx.contains(addr)
    }
    fn selfdestruct(&mut self, contract: &Address, beneficiary: &Address) {
        // Balance sweep first (matches default impl).
        let bal = self.balance(contract);
        if bal > 0 {
            let _ = self.transfer(contract, beneficiary, bal);
        }
        // Always enqueue — the executor decides at end-of-tx whether
        // this becomes a full delete (created this tx) or a no-op
        // (pre-existing contract; balance sweep already applied).
        self.pending_destructs.push((*contract, *beneficiary));
    }
}

impl Default for MockZvmHost {
    fn default() -> Self { Self::new() }
}
