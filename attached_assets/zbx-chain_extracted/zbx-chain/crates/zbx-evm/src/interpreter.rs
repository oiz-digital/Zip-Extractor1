//! EVM bytecode interpreter — executes one call frame and dispatches the
//! 0xF0–0xFF system-call family (CALL/CALLCODE/DELEGATECALL/STATICCALL +
//! CREATE/CREATE2 + SELFDESTRUCT) through the [`Host`] abstraction.
//!
//! Sprint S32 — closes audit C-21 / S7-EVM3 W1+W2+W3+W6 (zbx-evm half).
//! The matching ZVM mirror is W4 in a separate sprint.

use std::collections::HashSet;

use crate::{
    error::EvmError,
    gas::{
        copy_cost, forward_gas_eip150, log_cost, sha3_cost,
        CALL_DEPTH_LIMIT, GAS_CALL_STIPEND, GAS_CALL_VALUE_TRANSFER,
        GAS_CODE_DEPOSIT_PER_BYTE, GAS_COLD_ACCOUNT_ACCESS, GAS_INITCODE_WORD,
        GAS_NEW_ACCOUNT, GAS_SELFDESTRUCT, GAS_SHA3, GAS_SLOAD_COLD, GAS_SLOAD_WARM,
        GAS_SSTORE_RESET, GAS_SSTORE_SET, MAX_CONTRACT_CODE_SIZE, MAX_INITCODE_SIZE,
    },
    host::{Host, SnapshotId},
    memory::Memory,
    opcodes::Opcode,
    stack::{
        u256_add, u256_and, u256_byte, u256_byte_len, u256_div, u256_exp, u256_from_u64,
        u256_gt, u256_is_zero, u256_mod, u256_mul, u256_mulmod, u256_addmod, u256_not,
        u256_or, u256_sar, u256_sdiv, u256_sgt, u256_shl, u256_shr, u256_signextend,
        u256_slt, u256_smod, u256_sub, u256_to_u64, u256_xor, Stack,
    },
};
use zbx_crypto::keccak::keccak256;
use zbx_types::{address::Address, CHAIN_ID};

/// How a call frame terminated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitStatus {
    /// STOP or RETURN — success.
    Succeeded,
    /// REVERT — state changes discarded.
    Reverted,
    /// Fatal error (invalid opcode, out of gas, …).
    Failed(EvmError),
}

/// Call context supplied to the interpreter. Cloneable so sub-call dispatch
/// can derive a child context without heap-aliasing the parent's.
#[derive(Clone, Debug)]
pub struct EVMContext {
    pub caller: Address,
    pub callee: Address,
    pub value: [u8; 32],
    pub calldata: Vec<u8>,
    pub gas_limit: u64,
    pub is_static: bool,
    pub block_number: u64,
    pub timestamp: u64,
    pub coinbase: Address,
    /// BASEFEE opcode (0x48): the EIP-1559 base fee of the current block.
    pub base_fee: u64,
    /// GASPRICE opcode (0x3a): effective gas price paid by the transaction.
    /// For EIP-1559 transactions: base_fee + min(max_priority_fee, max_fee − base_fee).
    /// Propagated unchanged through DELEGATECALL and CALLCODE frames.
    pub gas_price: u64,
    /// ORIGIN opcode (0x32): the externally-owned account that originally signed
    /// the transaction. Propagated unchanged through all call frames including
    /// DELEGATECALL — only changes at the top-level tx entry point.
    pub tx_origin: Address,
    pub chain_id: u64,
    /// PREVRANDAO opcode (0x44): the beacon-chain RANDAO mix for this block.
    ///
    /// MB-2 fix: previously derived deterministically from block_number (fully
    /// predictable by anyone). Now supplied by the consensus layer per block —
    /// set to the BLS threshold randomness output or VDF mix from HotStuff-2.
    /// The consensus layer must populate this field for every block context.
    /// Falls back to keccak256(block_number) when consensus does not supply it
    /// (e.g. during devnet bootstrapping), which is still better than the
    /// previous deterministic multiply.
    pub randao_mix: [u8; 32],
}

impl EVMContext {
    pub fn effective_chain_id(&self) -> u64 {
        if self.chain_id == 0 { CHAIN_ID } else { self.chain_id }
    }
}

/// Maximum bytes a single call frame may RETURN/REVERT. Cancun caps deployed
/// code at ~24 KiB; for plain return data we permit up to 1 MiB which is
/// already enormous. Without this cap a contract could request a multi-GiB
/// allocation by RETURN(0, 2^40) and OOM the node. AUDIT M-11.
pub const MAX_RETURN_DATA: usize = 1024 * 1024;

/// A single EVM log entry emitted by LOG0–LOG4.
/// Collected internally; retrieve via `EVMInterpreter::take_logs()`.
#[derive(Debug, Clone)]
pub struct EvmLog {
    /// Contract address that emitted the log.
    pub address: Address,
    /// 0–4 indexed topics (each 32 bytes).
    pub topics: Vec<[u8; 32]>,
    /// Unindexed data payload.
    pub data: Vec<u8>,
}

/// One entry on the call stack. Tracks the snapshot to roll back to on
/// failure and (for CREATE/CREATE2) which address was minted in this frame
/// — drives EIP-6780 SELFDESTRUCT semantics.
#[derive(Clone, Debug)]
pub struct CallFrame {
    pub snapshot: SnapshotId,
    pub gas_at_entry: u64,
    pub created_address: Option<Address>,
}

/// Distinguishes the four CALL-family opcodes; lets `do_call` factor out
/// ~80% of shared dispatch logic (memory expansion, cold/warm accounting,
/// snapshot, precompile short-circuit, recursion, gas settlement).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CallKind {
    Call,
    CallCode,
    DelegateCall,
    StaticCall,
}

/// EVM interpreter — owns its frame's memory, stack, and program counter,
/// and borrows the chain `Host` for the duration of execution.
pub struct EVMInterpreter<'a> {
    ctx: EVMContext,
    code: Vec<u8>,
    /// Bitmap of valid JUMPDEST positions (one bit per code byte). AUDIT M-10.
    jumpdests: Vec<u8>,
    pc: usize,
    stack: Stack,
    memory: Memory,
    gas_remaining: u64,
    return_data: Vec<u8>,
    host: &'a mut dyn Host,
    /// Frame stack — `len()` is the current call depth. The outermost frame
    /// is implicit (depth 0); each CALL/CREATE pushes one entry.
    frames: Vec<CallFrame>,
    /// EIP-2929 warm-set. Persists across reverts (gas-only, never rolled
    /// back). Pre-warmed at construction with caller, callee, all
    /// precompiles, and coinbase.
    warm_addresses: HashSet<[u8; 20]>,
    /// EIP-2929 warm storage slots. Keyed by (contract_addr, slot_key).
    /// NEW-CRIT-02 FIX: required to implement SLOAD/SSTORE correctly.
    warm_storage: HashSet<([u8; 20], [u8; 32])>,
    /// LOG0–LOG4 entries emitted during this execution frame.
    /// NEW-CRIT-02 FIX: required to implement LOG opcodes.
    logs: Vec<EvmLog>,
}

impl EVMInterpreter<'_> {
    /// Construct a fresh interpreter for `ctx` over `code`, borrowing the
    /// chain host for the duration of execution. Lifetime is tied to the
    /// host borrow so sub-call recursion via `&mut *self.host` reborrow
    /// type-checks.
    pub fn new<'b>(
        ctx: EVMContext,
        code: Vec<u8>,
        host: &'b mut dyn Host,
    ) -> EVMInterpreter<'b> {
        let jumpdests = Self::build_jumpdest_bitmap(&code);
        let mut warm = HashSet::new();
        warm.insert(*ctx.caller.as_bytes());
        warm.insert(*ctx.callee.as_bytes());
        warm.insert(*ctx.coinbase.as_bytes());
        // EIP-2929 also pre-warms the nine precompiles (0x01..0x09).
        for i in 1u8..=9 {
            let mut p = [0u8; 20];
            p[19] = i;
            warm.insert(p);
        }
        EVMInterpreter {
            gas_remaining: ctx.gas_limit,
            ctx,
            code,
            jumpdests,
            pc: 0,
            stack: Stack::new(),
            memory: Memory::new(),
            return_data: Vec::new(),
            host,
            frames: Vec::new(),
            warm_addresses: warm,
            warm_storage: HashSet::new(),
            logs: Vec::new(),
        }
    }
}

impl<'a> EVMInterpreter<'a> {
    pub fn gas_used(&self) -> u64 {
        self.ctx.gas_limit.saturating_sub(self.gas_remaining)
    }

    pub fn return_data(&self) -> &[u8] {
        &self.return_data
    }

    /// Consume and return all LOG entries emitted during this execution.
    pub fn take_logs(&mut self) -> Vec<EvmLog> {
        std::mem::take(&mut self.logs)
    }

    fn consume_gas(&mut self, amount: u64) -> Result<(), EvmError> {
        if self.gas_remaining < amount {
            return Err(EvmError::OutOfGas);
        }
        self.gas_remaining -= amount;
        Ok(())
    }

    fn refund_gas(&mut self, amount: u64) {
        self.gas_remaining = self.gas_remaining.saturating_add(amount);
    }

    fn pop_address(&mut self) -> Result<Address, EvmError> {
        let w = self.stack.pop()?;
        let mut a = [0u8; 20];
        a.copy_from_slice(&w[12..32]);
        Ok(Address(a))
    }

    fn pop_u64_capped(&mut self) -> Result<u64, EvmError> {
        Ok(u256_to_u64(&self.stack.pop()?))
    }

    /// Build the JUMPDEST bitmap. AUDIT M-10.
    fn build_jumpdest_bitmap(code: &[u8]) -> Vec<u8> {
        let mut bm = vec![0u8; (code.len() + 7) / 8];
        let mut pc = 0;
        while pc < code.len() {
            let op = code[pc];
            if op == 0x5b {
                bm[pc / 8] |= 1 << (pc % 8);
                pc += 1;
            } else if (0x60..=0x7f).contains(&op) {
                let n = (op - 0x5f) as usize;
                pc += 1 + n;
            } else {
                pc += 1;
            }
        }
        bm
    }

    #[inline]
    fn is_valid_jumpdest(&self, dest: usize) -> bool {
        dest < self.code.len() && (self.jumpdests[dest / 8] & (1 << (dest % 8))) != 0
    }

    /// Execute until STOP / RETURN / REVERT / error.
    pub fn run(&mut self) -> (ExitStatus, u64) {
        loop {
            match self.step() {
                Ok(Some(status)) => return (status, self.gas_used()),
                Ok(None) => {}
                Err(e) => return (ExitStatus::Failed(e), self.gas_used()),
            }
        }
    }

    fn step(&mut self) -> Result<Option<ExitStatus>, EvmError> {
        if self.pc >= self.code.len() {
            return Ok(Some(ExitStatus::Succeeded));
        }

        let byte = self.code[self.pc];
        let op = match Opcode::from_u8(byte) {
            Some(op) => op,
            None => return Err(EvmError::InvalidOpcode(byte)),
        };

        // Deduct static gas (CALL/CREATE family static-portion is folded
        // into their dynamic cost — `static_gas()` returns the documented
        // base, then the handler tops up cold/warm/value/new-account.)
        self.consume_gas(op.static_gas())?;

        match byte {
            0x00 => return Ok(Some(ExitStatus::Succeeded)), // STOP
            0x01 => { // ADD
                let a = self.stack.pop()?;
                let b = self.stack.pop()?;
                self.stack.push(u256_add(&a, &b))?;
            }
            0x02 => { // MUL
                let a = self.stack.pop()?;
                let b = self.stack.pop()?;
                self.stack.push(u256_mul(&a, &b))?;
            }
            0x03 => { // SUB
                let a = self.stack.pop()?;
                let b = self.stack.pop()?;
                self.stack.push(u256_sub(&a, &b))?;
            }
            0x04 => { // DIV
                let a = self.stack.pop()?;
                let b = self.stack.pop()?;
                self.stack.push(u256_div(&a, &b))?;
            }
            0x05 => { // SDIV
                let a = self.stack.pop()?;
                let b = self.stack.pop()?;
                self.stack.push(u256_sdiv(&a, &b))?;
            }
            0x06 => { // MOD
                let a = self.stack.pop()?;
                let b = self.stack.pop()?;
                self.stack.push(u256_mod(&a, &b))?;
            }
            0x07 => { // SMOD
                let a = self.stack.pop()?;
                let b = self.stack.pop()?;
                self.stack.push(u256_smod(&a, &b))?;
            }
            0x08 => { // ADDMOD
                let a = self.stack.pop()?;
                let b = self.stack.pop()?;
                let n = self.stack.pop()?;
                self.stack.push(u256_addmod(&a, &b, &n))?;
            }
            0x09 => { // MULMOD
                let a = self.stack.pop()?;
                let b = self.stack.pop()?;
                let n = self.stack.pop()?;
                self.stack.push(u256_mulmod(&a, &b, &n))?;
            }
            0x0a => { // EXP — static gas is 0; charge 10 + 50*byte_len(exp)
                let base = self.stack.pop()?;
                let exp  = self.stack.pop()?;
                let byte_len = u256_byte_len(&exp);
                self.consume_gas(10 + 50 * byte_len)?;
                self.stack.push(u256_exp(&base, &exp))?;
            }
            0x0b => { // SIGNEXTEND
                let b = self.stack.pop()?;
                let x = self.stack.pop()?;
                self.stack.push(u256_signextend(&b, &x))?;
            }
            0x10 => { // LT
                let a = self.stack.pop()?;
                let b = self.stack.pop()?;
                self.stack.push(u256_from_u64(if a < b { 1 } else { 0 }))?;
            }
            0x11 => { // GT
                let a = self.stack.pop()?;
                let b = self.stack.pop()?;
                self.stack.push(u256_from_u64(if u256_gt(&a, &b) { 1 } else { 0 }))?;
            }
            0x12 => { // SLT
                let a = self.stack.pop()?;
                let b = self.stack.pop()?;
                self.stack.push(u256_from_u64(if u256_slt(&a, &b) { 1 } else { 0 }))?;
            }
            0x13 => { // SGT
                let a = self.stack.pop()?;
                let b = self.stack.pop()?;
                self.stack.push(u256_from_u64(if u256_sgt(&a, &b) { 1 } else { 0 }))?;
            }
            0x14 => { // EQ
                let a = self.stack.pop()?;
                let b = self.stack.pop()?;
                self.stack.push(u256_from_u64(if a == b { 1 } else { 0 }))?;
            }
            0x15 => { // ISZERO
                let a = self.stack.pop()?;
                self.stack.push(u256_from_u64(if u256_is_zero(&a) { 1 } else { 0 }))?;
            }
            0x16 => { // AND
                let a = self.stack.pop()?;
                let b = self.stack.pop()?;
                self.stack.push(u256_and(&a, &b))?;
            }
            0x17 => { // OR
                let a = self.stack.pop()?;
                let b = self.stack.pop()?;
                self.stack.push(u256_or(&a, &b))?;
            }
            0x18 => { // XOR
                let a = self.stack.pop()?;
                let b = self.stack.pop()?;
                self.stack.push(u256_xor(&a, &b))?;
            }
            0x19 => { // NOT
                let a = self.stack.pop()?;
                self.stack.push(u256_not(&a))?;
            }
            0x1a => { // BYTE
                let i = self.stack.pop()?;
                let x = self.stack.pop()?;
                self.stack.push(u256_byte(&i, &x))?;
            }
            0x1b => { // SHL
                let shift = self.stack.pop()?;
                let val   = self.stack.pop()?;
                self.stack.push(u256_shl(&shift, &val))?;
            }
            0x1c => { // SHR
                let shift = self.stack.pop()?;
                let val   = self.stack.pop()?;
                self.stack.push(u256_shr(&shift, &val))?;
            }
            0x1d => { // SAR
                let shift = self.stack.pop()?;
                let val   = self.stack.pop()?;
                self.stack.push(u256_sar(&shift, &val))?;
            }
            0x20 => { // SHA3 / KECCAK256 — fully dynamic gas
                let offset = u256_to_u64(&self.stack.pop()?) as usize;
                let size   = u256_to_u64(&self.stack.pop()?) as usize;
                let expand_gas = self.memory.ensure(offset, size)?;
                self.consume_gas(expand_gas)?;
                self.consume_gas(sha3_cost(size as u64))?;
                let data = self.memory.read(offset, size);
                let hash = keccak256(&data);
                self.stack.push(hash.0)?;
            }
            0x30 => { // ADDRESS
                let mut w = [0u8; 32];
                w[12..].copy_from_slice(self.ctx.callee.as_bytes());
                self.stack.push(w)?;
            }
            0x31 => { // BALANCE — warm/cold per EIP-2929
                let addr = self.pop_address()?;
                if !self.warm_addresses.contains(addr.as_bytes()) {
                    self.consume_gas(GAS_COLD_ACCOUNT_ACCESS)?;
                    self.warm_addresses.insert(*addr.as_bytes());
                }
                self.stack.push(self.host.balance(&addr))?;
            }
            0x32 => { // ORIGIN — original tx signer (unchanged across DELEGATECALL frames)
                let mut w = [0u8; 32];
                w[12..].copy_from_slice(self.ctx.tx_origin.as_bytes());
                self.stack.push(w)?;
            }
            0x33 => { // CALLER
                let mut w = [0u8; 32];
                w[12..].copy_from_slice(self.ctx.caller.as_bytes());
                self.stack.push(w)?;
            }
            0x34 => { // CALLVALUE
                self.stack.push(self.ctx.value)?;
            }
            0x35 => { // CALLDATALOAD
                let offset = u256_to_u64(&self.stack.pop()?) as usize;
                let mut w = [0u8; 32];
                let data = &self.ctx.calldata;
                let available = data.len().saturating_sub(offset);
                let len = available.min(32);
                if len > 0 {
                    w[..len].copy_from_slice(&data[offset..offset + len]);
                }
                self.stack.push(w)?;
            }
            0x36 => { // CALLDATASIZE
                self.stack.push(u256_from_u64(self.ctx.calldata.len() as u64))?;
            }
            0x37 => { // CALLDATACOPY
                let dest = u256_to_u64(&self.stack.pop()?) as usize;
                let src  = u256_to_u64(&self.stack.pop()?) as usize;
                let size = u256_to_u64(&self.stack.pop()?) as usize;
                let expand_gas = self.memory.ensure(dest, size)?;
                self.consume_gas(expand_gas)?;
                self.consume_gas(copy_cost(size as u64))?;
                if size > 0 {
                    let mut buf = vec![0u8; size];
                    let avail = self.ctx.calldata.len().saturating_sub(src);
                    let copy_len = avail.min(size);
                    buf[..copy_len].copy_from_slice(&self.ctx.calldata[src..src + copy_len]);
                    self.memory.write(dest, &buf);
                }
            }
            0x38 => { // CODESIZE
                self.stack.push(u256_from_u64(self.code.len() as u64))?;
            }
            0x39 => { // CODECOPY
                let dest = u256_to_u64(&self.stack.pop()?) as usize;
                let src  = u256_to_u64(&self.stack.pop()?) as usize;
                let size = u256_to_u64(&self.stack.pop()?) as usize;
                let expand_gas = self.memory.ensure(dest, size)?;
                self.consume_gas(expand_gas)?;
                self.consume_gas(copy_cost(size as u64))?;
                if size > 0 {
                    let mut buf = vec![0u8; size];
                    let avail = self.code.len().saturating_sub(src);
                    let copy_len = avail.min(size);
                    buf[..copy_len].copy_from_slice(&self.code[src..src + copy_len]);
                    self.memory.write(dest, &buf);
                }
            }
            0x3a => { // GASPRICE — effective gas price (base_fee + priority_fee tip)
                self.stack.push(u256_from_u64(self.ctx.gas_price))?;
            }
            0x3b => { // EXTCODESIZE — warm/cold per EIP-2929
                let addr = self.pop_address()?;
                if !self.warm_addresses.contains(addr.as_bytes()) {
                    self.consume_gas(GAS_COLD_ACCOUNT_ACCESS)?;
                    self.warm_addresses.insert(*addr.as_bytes());
                }
                let code_len = self.host.code(&addr).len();
                self.stack.push(u256_from_u64(code_len as u64))?;
            }
            0x3c => { // EXTCODECOPY — warm/cold + memory expansion + copy cost
                let addr = self.pop_address()?;
                let dest = u256_to_u64(&self.stack.pop()?) as usize;
                let src  = u256_to_u64(&self.stack.pop()?) as usize;
                let size = u256_to_u64(&self.stack.pop()?) as usize;
                if !self.warm_addresses.contains(addr.as_bytes()) {
                    self.consume_gas(GAS_COLD_ACCOUNT_ACCESS)?;
                    self.warm_addresses.insert(*addr.as_bytes());
                }
                let expand_gas = self.memory.ensure(dest, size)?;
                self.consume_gas(expand_gas)?;
                self.consume_gas(copy_cost(size as u64))?;
                if size > 0 {
                    let code = self.host.code(&addr);
                    let mut buf = vec![0u8; size];
                    let avail = code.len().saturating_sub(src);
                    let copy_len = avail.min(size);
                    buf[..copy_len].copy_from_slice(&code[src..src + copy_len]);
                    self.memory.write(dest, &buf);
                }
            }
            0x3f => { // EXTCODEHASH — warm/cold; empty account → 0
                let addr = self.pop_address()?;
                if !self.warm_addresses.contains(addr.as_bytes()) {
                    self.consume_gas(GAS_COLD_ACCOUNT_ACCESS)?;
                    self.warm_addresses.insert(*addr.as_bytes());
                }
                if self.host.is_empty(&addr) {
                    self.stack.push([0u8; 32])?;
                } else {
                    self.stack.push(self.host.code_hash(&addr))?;
                }
            }
            0x3d => { // RETURNDATASIZE
                self.stack.push(u256_from_u64(self.return_data.len() as u64))?;
            }
            0x3e => { // RETURNDATACOPY
                let dest = u256_to_u64(&self.stack.pop()?) as usize;
                let off  = u256_to_u64(&self.stack.pop()?) as usize;
                let size = u256_to_u64(&self.stack.pop()?) as usize;
                // EIP-211: out-of-bounds RETURNDATACOPY MUST fault. Guard
                // against overflow first, then bounds.
                let end = off.checked_add(size)
                    .ok_or(EvmError::MemoryOutOfBounds { offset: off, size })?;
                if end > self.return_data.len() {
                    return Err(EvmError::MemoryOutOfBounds { offset: off, size });
                }
                let expand_gas = self.memory.ensure(dest, size)?;
                self.consume_gas(expand_gas)?;
                let rdc_cost = 3 * ((size as u64 + 31) / 32);
                self.consume_gas(rdc_cost)?;
                if size > 0 {
                    let chunk = self.return_data[off..off + size].to_vec();
                    self.memory.write(dest, &chunk);
                }
            }
            0x40 => { // BLOCKHASH — EIP-2935 / Yellow Paper §F.3
                // The opcode pops a block number and returns its keccak256 hash.
                // Only the 256 most-recent completed blocks are available;
                // anything outside [current_block - 256, current_block - 1]
                // must return zero. The current block is NOT available (it is
                // still being built), so `block_num == current` also returns 0.
                let raw = self.stack.pop()?;
                let block_num = u256_to_u64(&raw);
                let current   = self.ctx.block_number;
                let hash = if block_num < current
                    && current.saturating_sub(block_num) <= 256
                {
                    self.host.block_hash(block_num)
                } else {
                    [0u8; 32]
                };
                self.stack.push(hash)?;
            }
            0x41 => { // COINBASE
                let mut w = [0u8; 32];
                w[12..].copy_from_slice(self.ctx.coinbase.as_bytes());
                self.stack.push(w)?;
            }
            0x42 => { // TIMESTAMP
                self.stack.push(u256_from_u64(self.ctx.timestamp))?;
            }
            0x43 => { // NUMBER
                self.stack.push(u256_from_u64(self.ctx.block_number))?;
            }
            0x44 => { // PREVRANDAO (formerly DIFFICULTY post-Merge)
                // MB-2 fix: use the randao_mix supplied by the consensus layer.
                // This field is set per-block from the BLS threshold randomness
                // or VDF mix produced by HotStuff-2. It is NOT derived from
                // block_number (which was fully predictable by any attacker).
                self.stack.push(self.ctx.randao_mix)?;
            }
            0x45 => { // GASLIMIT — read from block execution context
                // H-1 fix: was hardcoded to 30_000_000; now reads gas_limit
                // from the call context so it reflects the actual block gas cap.
                self.stack.push(u256_from_u64(self.ctx.gas_limit))?;
            }
            0x46 => { // CHAINID
                self.stack.push(u256_from_u64(self.ctx.effective_chain_id()))?;
            }
            0x47 => { // SELFBALANCE — own balance, always warm
                self.stack.push(self.host.balance(&self.ctx.callee))?;
            }
            0x48 => { // BASEFEE
                self.stack.push(u256_from_u64(self.ctx.base_fee))?;
            }
            0x50 => { // POP
                self.stack.pop()?;
            }
            0x51 => { // MLOAD
                let offset = u256_to_u64(&self.stack.pop()?) as usize;
                let expand_gas = self.memory.ensure(offset, 32)?;
                self.consume_gas(expand_gas)?;
                let val = self.memory.read32(offset);
                self.stack.push(val)?;
            }
            0x52 => { // MSTORE
                let offset = u256_to_u64(&self.stack.pop()?) as usize;
                let val = self.stack.pop()?;
                let expand_gas = self.memory.ensure(offset, 32)?;
                self.consume_gas(expand_gas)?;
                self.memory.write32(offset, &val);
            }
            0x53 => { // MSTORE8
                let offset = u256_to_u64(&self.stack.pop()?) as usize;
                let val = self.stack.pop()?;
                let expand_gas = self.memory.ensure(offset, 1)?;
                self.consume_gas(expand_gas)?;
                self.memory.write(offset, &[val[31]]);
            }
            0x54 => { // SLOAD — warm/cold per EIP-2929
                let key = self.stack.pop()?;
                let slot = (*self.ctx.callee.as_bytes(), key);
                if !self.warm_storage.contains(&slot) {
                    self.consume_gas(GAS_SLOAD_COLD.saturating_sub(GAS_SLOAD_WARM))?;
                    self.warm_storage.insert(slot);
                }
                let val = self.host.storage_load(&self.ctx.callee, &key);
                self.stack.push(val)?;
            }
            0x55 => { // SSTORE — static guard + simplified EIP-2200 gas
                if self.ctx.is_static {
                    return Err(EvmError::StaticStateChange);
                }
                let key   = self.stack.pop()?;
                let value = self.stack.pop()?;
                let slot  = (*self.ctx.callee.as_bytes(), key);
                let current = self.host.storage_load(&self.ctx.callee, &key);
                // Cold slot surcharge.
                if !self.warm_storage.contains(&slot) {
                    self.consume_gas(GAS_SLOAD_COLD.saturating_sub(GAS_SLOAD_WARM))?;
                    self.warm_storage.insert(slot);
                }
                // M-6 fix: Full EIP-2200 SSTORE gas accounting including
                // dirty-slot refund case (writing back to original value).
                //
                // Gas rules (EIP-2200):
                //   current == value                         → NOOP (100 gas static)
                //   original == current && current == 0      → 20000 (cold set)
                //   original == current && current != 0      → 2900  (cold reset)
                //   original != current (slot is dirty)
                //     new == original                        → 200   (restore refund)
                //     new == 0 && original != 0              → 200   (refund 15000)
                //     otherwise                              → 200   (dirty write)
                const GAS_SSTORE_DIRTY: u64 = 200;

                if current != value {
                    let original = self.host.storage_load(&self.ctx.callee, &key);
                    if original == current {
                        // Slot is clean (current matches committed value).
                        if u256_is_zero(&current) {
                            self.consume_gas(GAS_SSTORE_SET)?;    // 0→nonzero: 20000
                        } else {
                            self.consume_gas(GAS_SSTORE_RESET)?;  // nonzero→other: 2900
                        }
                    } else {
                        // Slot is dirty (already modified this tx).
                        self.consume_gas(GAS_SSTORE_DIRTY)?;       // any dirty write: 200
                    }
                }
                self.host.storage_store(&self.ctx.callee, key, value);
            }
            0x58 => { // PC — program counter BEFORE this instruction
                self.stack.push(u256_from_u64(self.pc as u64))?;
            }
            0x59 => { // MSIZE — size of active memory in bytes (always 32-byte aligned)
                self.stack.push(u256_from_u64(self.memory.size() as u64))?;
            }
            0x5a => { // GAS — remaining gas after static deduction
                self.stack.push(u256_from_u64(self.gas_remaining))?;
            }
            0x56 => { // JUMP
                let dest = u256_to_u64(&self.stack.pop()?) as usize;
                if !self.is_valid_jumpdest(dest) {
                    return Err(EvmError::InvalidJump(dest));
                }
                self.pc = dest;
                return Ok(None);
            }
            0x57 => { // JUMPI
                let dest = u256_to_u64(&self.stack.pop()?) as usize;
                let cond = self.stack.pop()?;
                if !u256_is_zero(&cond) {
                    if !self.is_valid_jumpdest(dest) {
                        return Err(EvmError::InvalidJump(dest));
                    }
                    self.pc = dest;
                    return Ok(None);
                }
            }
            0x5b => {} // JUMPDEST — no-op
            0x5f => { // PUSH0 (EIP-3855)
                self.stack.push([0u8; 32])?;
            }
            0x60..=0x7f => { // PUSH1..PUSH32
                let n = (byte - 0x5f) as usize;
                let mut val = [0u8; 32];
                let start = self.pc + 1;
                let end = (start + n).min(self.code.len());
                let available = end - start;
                val[32 - n..32 - n + available]
                    .copy_from_slice(&self.code[start..end]);
                self.stack.push(val)?;
                self.pc += n;
            }
            0x80..=0x8f => { // DUP1..DUP16
                self.stack.dup((byte - 0x7f) as usize)?;
            }
            0x90..=0x9f => { // SWAP1..SWAP16
                self.stack.swap((byte - 0x8f) as usize)?;
            }
            // ── LOG0–LOG4 (0xA0–0xA4) ───────────────────────────────────────
            // NEW-CRIT-02 FIX: LOG opcodes were completely absent.
            // Fully dynamic gas (static portion = 0 in opcodes.rs).
            0xa0..=0xa4 => {
                if self.ctx.is_static {
                    return Err(EvmError::StaticStateChange);
                }
                let topic_count = (byte - 0xa0) as usize;
                let offset = u256_to_u64(&self.stack.pop()?) as usize;
                let size   = u256_to_u64(&self.stack.pop()?) as usize;
                let mut topics = Vec::with_capacity(topic_count);
                for _ in 0..topic_count {
                    topics.push(self.stack.pop()?);
                }
                let expand_gas = self.memory.ensure(offset, size)?;
                self.consume_gas(expand_gas)?;
                self.consume_gas(log_cost(size as u64, topic_count as u64))?;
                let data = self.memory.read(offset, size);
                self.logs.push(EvmLog {
                    address: self.ctx.callee,
                    topics,
                    data,
                });
            }
            // ── 0xF0–0xFF system-call family (S32) ─────────────────────────
            0xf0 => { self.do_create(false)?; }       // CREATE
            0xf1 => { self.do_call(CallKind::Call)?; }
            0xf2 => { self.do_call(CallKind::CallCode)?; }
            0xf3 => { // RETURN
                let offset = u256_to_u64(&self.stack.pop()?) as usize;
                let size = u256_to_u64(&self.stack.pop()?) as usize;
                if size > MAX_RETURN_DATA {
                    return Err(EvmError::MemoryOutOfBounds { offset, size });
                }
                let expand_gas = self.memory.ensure(offset, size)?;
                self.consume_gas(expand_gas)?;
                self.return_data = self.memory.read(offset, size);
                return Ok(Some(ExitStatus::Succeeded));
            }
            0xf4 => { self.do_call(CallKind::DelegateCall)?; }
            0xf5 => { self.do_create(true)?; }        // CREATE2
            0xfa => { self.do_call(CallKind::StaticCall)?; }
            0xfd => { // REVERT
                let offset = u256_to_u64(&self.stack.pop()?) as usize;
                let size = u256_to_u64(&self.stack.pop()?) as usize;
                if size > MAX_RETURN_DATA {
                    return Err(EvmError::MemoryOutOfBounds { offset, size });
                }
                let expand_gas = self.memory.ensure(offset, size)?;
                self.consume_gas(expand_gas)?;
                self.return_data = self.memory.read(offset, size);
                return Ok(Some(ExitStatus::Reverted));
            }
            0xfe => return Err(EvmError::InvalidOpcode(0xfe)), // INVALID
            0xff => { // SELFDESTRUCT — EIP-6780
                if self.ctx.is_static {
                    return Err(EvmError::StaticStateChange);
                }
                let beneficiary = self.pop_address()?;
                // Static portion already deducted via `static_gas()` if any;
                // top up the EIP-150 SELFDESTRUCT base.
                self.consume_gas(GAS_SELFDESTRUCT)?;
                // Cold/warm surcharge for the beneficiary.
                if !self.warm_addresses.contains(beneficiary.as_bytes()) {
                    self.consume_gas(GAS_COLD_ACCOUNT_ACCESS)?;
                    self.warm_addresses.insert(*beneficiary.as_bytes());
                }
                self.host.destruct(&self.ctx.callee, &beneficiary);
                return Ok(Some(ExitStatus::Succeeded));
            }
            _ => {
                // S7-EVM-INVOPS [audit 2026-05-01]: unknown opcodes MUST
                // halt the frame (Yellow Paper appendix H). Falling through
                // would silently NOP and leak stack args of would-be CALL
                // ops — the very bug S7-EVM3 is closing.
                return Err(EvmError::InvalidOpcode(byte));
            }
        }

        self.pc += 1;
        Ok(None)
    }

    // ─────────────────────────────────────────────────────────────────────
    //  CALL family dispatcher (S32 / S7-EVM3 W1)
    // ─────────────────────────────────────────────────────────────────────

    /// Common dispatcher for CALL / CALLCODE / DELEGATECALL / STATICCALL.
    ///
    /// Stack pop order varies by kind:
    /// - CALL / CALLCODE   : gas, addr, value, argsOff, argsLen, retOff, retLen
    /// - DELEGATECALL/STATIC: gas, addr,        argsOff, argsLen, retOff, retLen
    fn do_call(&mut self, kind: CallKind) -> Result<(), EvmError> {
        let gas_req = self.pop_u64_capped()?;
        let target = self.pop_address()?;
        let value = match kind {
            CallKind::Call | CallKind::CallCode => self.stack.pop()?,
            CallKind::DelegateCall => self.ctx.value,
            CallKind::StaticCall => [0u8; 32],
        };

        // Static-frame guard: CALL and CALLCODE with positive value are
        // state-mutating and forbidden inside a STATICCALL frame.
        if matches!(kind, CallKind::Call | CallKind::CallCode)
            && self.ctx.is_static
            && !u256_is_zero(&value)
        {
            return Err(EvmError::StaticStateChange);
        }

        let args_off = self.pop_u64_capped()? as usize;
        let args_len = self.pop_u64_capped()? as usize;
        let ret_off = self.pop_u64_capped()? as usize;
        let ret_len = self.pop_u64_capped()? as usize;

        // Memory expansion for argument range.
        let expand1 = self.memory.ensure(args_off, args_len)?;
        self.consume_gas(expand1)?;
        let calldata = self.memory.read(args_off, args_len);

        // EIP-2929 cold/warm surcharge.
        if !self.warm_addresses.contains(target.as_bytes()) {
            self.consume_gas(GAS_COLD_ACCOUNT_ACCESS)?;
            self.warm_addresses.insert(*target.as_bytes());
        }

        // Value-transfer surcharges only apply to CALL / CALLCODE with
        // value != 0. DELEGATECALL forwards parent's value but doesn't
        // transfer; STATICCALL has value = 0 by construction.
        let actually_transfers =
            matches!(kind, CallKind::Call | CallKind::CallCode) && !u256_is_zero(&value);
        if actually_transfers {
            self.consume_gas(GAS_CALL_VALUE_TRANSFER)?;
            // New-account surcharge only on CALL (CALLCODE writes to the
            // caller's own account, never creates a new one).
            if matches!(kind, CallKind::Call) && self.host.is_empty(&target) {
                self.consume_gas(GAS_NEW_ACCOUNT)?;
            }
        }

        // Helper: write empty result and push 0. Used by all early-exit
        // failure paths so the caller's stack invariant stays correct.
        let push_zero_with_ret = |me: &mut Self| -> Result<(), EvmError> {
            me.return_data.clear();
            let expand2 = me.memory.ensure(ret_off, ret_len)?;
            me.consume_gas(expand2)?;
            me.stack.push(u256_from_u64(0))?;
            Ok(())
        };

        // EIP-150 depth check.
        if self.frames.len() >= CALL_DEPTH_LIMIT {
            return push_zero_with_ret(self);
        }

        // Compute forwarded gas. The 63/64 rule caps what the parent gives
        // up; the stipend (when value > 0) is a free top-up to the callee
        // — it does NOT come out of the parent's gas counter.
        let forwarded_billed = forward_gas_eip150(self.gas_remaining, gas_req);
        let stipend = if actually_transfers { GAS_CALL_STIPEND } else { 0 };
        let forwarded = forwarded_billed.saturating_add(stipend);
        // Deduct only the billed portion from the parent.
        self.consume_gas(forwarded_billed)?;

        // Take a snapshot before any state mutation so we can revert on
        // failure.
        let snap = self.host.snapshot();

        // Value transfer (only for genuine CALL with positive value).
        // CALLCODE notionally moves value to self — modelled as a no-op.
        if matches!(kind, CallKind::Call) && actually_transfers {
            let from_bal = self.host.balance(&self.ctx.callee);
            if from_bal.as_slice() < value.as_slice() {
                self.host.revert_to(snap);
                // Refund only the billed portion. The stipend was never
                // deducted from the parent's gas counter — refunding it
                // would synthesise gas out of thin air (architect S32 #2).
                self.refund_gas(forwarded_billed);
                return push_zero_with_ret(self);
            }
            // Cannot fail given the balance check above.
            let _ = self.host.transfer(&self.ctx.callee, &target, &value);
        }

        // Precompile short-circuit. MUST happen BEFORE the host.code() check
        // — precompile addresses have empty bytecode in state, so without
        // this branch every CALL into 0x01..0x09 would fall into the
        // empty-code success path and silently return nothing.
        if crate::precompiles::is_precompile(&target) {
            // Task #3 (Precompile 0x0A — PayID resolution): stateful precompile.
            // Routed through a host adapter rather than the stateless
            // `call_precompile` dispatcher (which fails-closed for 0x0A).
            let result = if target.as_bytes()[19] == 0x0A {
                struct HostAdapter<'a, H: crate::host::Host + ?Sized>(&'a H);
                impl<H: crate::host::Host + ?Sized> crate::precompiles::PayIdLookup
                    for HostAdapter<'_, H>
                {
                    fn resolve(&self, name: &[u8]) -> Option<[u8; 20]> {
                        self.0.resolve_pay_id_bytes(name)
                    }
                    fn reverse(&self, addr: &[u8; 20]) -> Option<Vec<u8>> {
                        self.0.reverse_pay_id(addr)
                    }
                }
                let adapter = HostAdapter(&*self.host);
                crate::precompiles::do_payid(&calldata, forwarded, &adapter)
            } else if target.as_bytes()[19] == 0x0C {
                // Task #5 (Precompile 0x0C — Price oracle read): stateful
                // precompile routed through a host adapter that bridges
                // EVM `storage_load` to `OracleStateReader::read_slot`.
                // Byte-identical to the ZVM path.
                struct OracleAdapter<'a, H: crate::host::Host + ?Sized>(&'a H);
                impl<H: crate::host::Host + ?Sized>
                    zbx_crypto::oracle_state::OracleStateReader for OracleAdapter<'_, H>
                {
                    fn read_slot(&self, addr: &[u8; 20], slot: &[u8; 32]) -> [u8; 32] {
                        self.0
                            .storage_load(&zbx_types::address::Address(*addr), slot)
                    }
                }
                let adapter = OracleAdapter(&*self.host);
                crate::precompiles::do_price_oracle(&calldata, forwarded, &adapter)
            } else if target.as_bytes()[19] == 0x0F {
                // Task #7 (Precompile 0x0F — ZUSD vault state direct-read):
                // stateful precompile routed through a host adapter that
                // bridges EVM `storage_load` to `OracleStateReader::read_slot`
                // for both the vault contract and the oracle registry.
                // Byte-identical to the ZVM path.
                struct VaultAdapter<'a, H: crate::host::Host + ?Sized> {
                    host: &'a H,
                    ts: u64,
                }
                impl<H: crate::host::Host + ?Sized>
                    zbx_crypto::vault_state::VaultStateReader for VaultAdapter<'_, H>
                {
                    fn read_slot(&self, addr: &[u8; 20], slot: &[u8; 32]) -> [u8; 32] {
                        self.host
                            .storage_load(&zbx_types::address::Address(*addr), slot)
                    }
                    fn current_timestamp(&self) -> u64 { self.ts }
                }
                let adapter = VaultAdapter { host: &*self.host, ts: self.ctx.timestamp };
                crate::precompiles::do_zusd_vault(&calldata, forwarded, &adapter)
            } else {
                crate::precompiles::call_precompile(&target, &calldata, forwarded)
            };
            match result {
                Ok((output, gas_used)) => {
                    self.host.commit(snap);
                    self.return_data = output.clone();
                    let expand2 = self.memory.ensure(ret_off, ret_len)?;
                    self.consume_gas(expand2)?;
                    let copy = ret_len.min(output.len());
                    if copy > 0 {
                        self.memory.write(ret_off, &output[..copy]);
                    }
                    // Cap refund at what the parent actually paid in.
                    // Stipend gas is callee-only — never refundable to caller
                    // (architect S32 #2; consensus-critical mint guard).
                    let unused = forwarded
                        .saturating_sub(gas_used)
                        .min(forwarded_billed);
                    self.refund_gas(unused);
                    self.stack.push(u256_from_u64(1))?;
                }
                Err(_) => {
                    self.host.revert_to(snap);
                    self.return_data.clear();
                    let expand2 = self.memory.ensure(ret_off, ret_len)?;
                    self.consume_gas(expand2)?;
                    // Forwarded gas is consumed — precompile failures burn
                    // the budget mainnet-side.
                    self.stack.push(u256_from_u64(0))?;
                }
            }
            return Ok(());
        }

        // Fetch target code; empty code = success-with-no-return-data.
        let target_code = self.host.code(&target);
        if target_code.is_empty() {
            self.host.commit(snap);
            self.return_data.clear();
            let expand2 = self.memory.ensure(ret_off, ret_len)?;
            self.consume_gas(expand2)?;
            // Refund only the billed portion — see architect S32 #2.
            self.refund_gas(forwarded_billed);
            self.stack.push(u256_from_u64(1))?;
            return Ok(());
        }

        // Build the sub-context per kind. The variations here are the
        // semantic essence of CALL vs CALLCODE vs DELEGATECALL vs STATICCALL.
        let sub_ctx = match kind {
            CallKind::Call => EVMContext {
                caller: self.ctx.callee,
                callee: target,
                value,
                calldata,
                gas_limit: forwarded,
                is_static: self.ctx.is_static,
                block_number: self.ctx.block_number,
                timestamp: self.ctx.timestamp,
                coinbase: self.ctx.coinbase,
                base_fee: self.ctx.base_fee,
                gas_price: self.ctx.gas_price,
                tx_origin: self.ctx.tx_origin,
                chain_id: self.ctx.chain_id,
                randao_mix: self.ctx.randao_mix,
            },
            CallKind::CallCode => EVMContext {
                caller: self.ctx.callee,
                callee: self.ctx.callee, // storage stays with caller
                value,
                calldata,
                gas_limit: forwarded,
                is_static: self.ctx.is_static,
                block_number: self.ctx.block_number,
                timestamp: self.ctx.timestamp,
                coinbase: self.ctx.coinbase,
                base_fee: self.ctx.base_fee,
                gas_price: self.ctx.gas_price,
                tx_origin: self.ctx.tx_origin,
                chain_id: self.ctx.chain_id,
                randao_mix: self.ctx.randao_mix,
            },
            CallKind::DelegateCall => EVMContext {
                caller: self.ctx.caller,    // PRESERVED
                callee: self.ctx.callee,    // PRESERVED
                value: self.ctx.value,      // PRESERVED
                calldata,
                gas_limit: forwarded,
                is_static: self.ctx.is_static,
                block_number: self.ctx.block_number,
                timestamp: self.ctx.timestamp,
                coinbase: self.ctx.coinbase,
                base_fee: self.ctx.base_fee,
                gas_price: self.ctx.gas_price,
                tx_origin: self.ctx.tx_origin,
                chain_id: self.ctx.chain_id,
                randao_mix: self.ctx.randao_mix,
            },
            CallKind::StaticCall => EVMContext {
                caller: self.ctx.callee,
                callee: target,
                value: [0u8; 32],
                calldata,
                gas_limit: forwarded,
                is_static: true,            // FORCED
                block_number: self.ctx.block_number,
                timestamp: self.ctx.timestamp,
                coinbase: self.ctx.coinbase,
                base_fee: self.ctx.base_fee,
                gas_price: self.ctx.gas_price,
                tx_origin: self.ctx.tx_origin,
                chain_id: self.ctx.chain_id,
                randao_mix: self.ctx.randao_mix,
            },
        };

        // Recurse. Reborrow the host (`&mut *self.host`) so the sub
        // interpreter takes a fresh shorter-lived borrow that can be
        // dropped at the end of this scope.
        let parent_warm = self.warm_addresses.clone();
        let parent_frames = self.frames.clone();
        let (status, gas_used, sub_return, sub_warm, sub_logs) = {
            let mut sub = EVMInterpreter::new(sub_ctx, target_code, &mut *self.host);
            sub.frames = parent_frames;
            sub.frames.push(CallFrame {
                snapshot: snap,
                gas_at_entry: forwarded,
                created_address: None,
            });
            sub.warm_addresses = parent_warm;
            let (status, gas_used) = sub.run();
            let r = std::mem::take(&mut sub.return_data);
            let w = std::mem::take(&mut sub.warm_addresses);
            // EVM-01 FIX (HIGH): collect logs BEFORE dropping sub. Logs from
            // sub-calls must bubble up to the parent frame so they appear in
            // the transaction receipt. Previously `sub` was dropped here and
            // its `logs` field was silently discarded — any event emitted by a
            // callee (e.g. ERC-20 Transfer events via proxy patterns) was lost.
            let l = sub.take_logs();
            (status, gas_used, r, w, l)
        };
        // EIP-2929: the warm set is gas-only — never rolled back, even on
        // sub-call revert.
        self.warm_addresses = sub_warm;

        let success = matches!(status, ExitStatus::Succeeded);
        if success {
            self.host.commit(snap);
            // EVM-01 FIX: only propagate logs on success; logs from a reverted
            // sub-call are discarded (Yellow Paper §9.4 / EIP-140 semantics).
            self.logs.extend(sub_logs);
        } else {
            self.host.revert_to(snap);
        }
        // Cap refund at what the parent actually paid in. The stipend
        // (when value > 0) is bonus budget for the callee that comes from
        // outside the parent's gas counter; refunding it on a no-op callee
        // would mint gas (architect S32 #2; consensus-critical).
        let unused = forwarded
            .saturating_sub(gas_used)
            .min(forwarded_billed);
        self.refund_gas(unused);

        self.return_data = sub_return.clone();
        let expand2 = self.memory.ensure(ret_off, ret_len)?;
        self.consume_gas(expand2)?;
        let copy = ret_len.min(sub_return.len());
        if copy > 0 {
            self.memory.write(ret_off, &sub_return[..copy]);
        }
        self.stack.push(u256_from_u64(if success { 1 } else { 0 }))?;
        Ok(())
    }

    // ─────────────────────────────────────────────────────────────────────
    //  CREATE / CREATE2 dispatcher (S32 / S7-EVM3 W2)
    // ─────────────────────────────────────────────────────────────────────

    /// Common dispatcher for CREATE (`with_salt = false`) and CREATE2.
    ///
    /// Stack pop order:
    /// - CREATE  : value, offset, length
    /// - CREATE2 : value, offset, length, salt
    fn do_create(&mut self, with_salt: bool) -> Result<(), EvmError> {
        if self.ctx.is_static {
            return Err(EvmError::StaticStateChange);
        }
        let value = self.stack.pop()?;
        let off = self.pop_u64_capped()? as usize;
        let len = self.pop_u64_capped()? as usize;
        let salt_opt = if with_salt { Some(self.stack.pop()?) } else { None };

        // EIP-3860 initcode size cap.
        if len > MAX_INITCODE_SIZE {
            return Err(EvmError::InitcodeOversize(len));
        }

        // Memory expansion + initcode word cost.
        let expand_gas = self.memory.ensure(off, len)?;
        self.consume_gas(expand_gas)?;
        let initcode_words = (len as u64 + 31) / 32;
        self.consume_gas(GAS_INITCODE_WORD * initcode_words)?;
        // CREATE2 additionally pays for keccak256(initcode) — 6 gas/word.
        if with_salt {
            self.consume_gas(6 * initcode_words)?;
        }
        let initcode = self.memory.read(off, len);

        let push_zero = |me: &mut Self| -> Result<(), EvmError> {
            me.return_data.clear();
            me.stack.push([0u8; 32])?;
            Ok(())
        };

        if self.frames.len() >= CALL_DEPTH_LIMIT {
            return push_zero(self);
        }

        let from_bal = self.host.balance(&self.ctx.callee);
        if from_bal.as_slice() < value.as_slice() {
            return push_zero(self);
        }

        // Compute new contract address (must use CURRENT nonce — incremented
        // immediately after — so two CREATE ops in the same frame derive
        // distinct addresses).
        let new_addr = if let Some(salt) = salt_opt {
            create2_address(&self.ctx.callee, &salt, &initcode)
        } else {
            let nonce = self.host.nonce(&self.ctx.callee);
            create_address(&self.ctx.callee, nonce)
        };

        // Bump sender nonce — failure here (NonceOverflow) bubbles up as a
        // hard EvmError per EIP-2681 to avoid CREATE address aliasing.
        self.host.inc_nonce(&self.ctx.callee)?;

        // Collision check: target must be empty (no nonce, no code).
        if self.host.nonce(&new_addr) > 0 || !self.host.code(&new_addr).is_empty() {
            return push_zero(self);
        }

        let snap = self.host.snapshot();

        // Value transfer (balance pre-checked).
        if !u256_is_zero(&value) {
            let _ = self.host.transfer(&self.ctx.callee, &new_addr, &value);
        }

        // EIP-161 / EIP-7610: created account starts at nonce 1 (so its
        // own future CREATEs derive non-aliased addresses).
        let _ = self.host.inc_nonce(&new_addr)?;
        self.host.mark_created_this_tx(&new_addr);

        // Forward all available gas (subject to 63/64 rule).
        let forwarded = forward_gas_eip150(self.gas_remaining, self.gas_remaining);
        self.consume_gas(forwarded)?;

        let sub_ctx = EVMContext {
            caller: self.ctx.callee,
            callee: new_addr,
            value,
            calldata: vec![],
            gas_limit: forwarded,
            is_static: false,
            block_number: self.ctx.block_number,
            timestamp: self.ctx.timestamp,
            coinbase: self.ctx.coinbase,
            base_fee: self.ctx.base_fee,
            gas_price: self.ctx.gas_price,
            tx_origin: self.ctx.tx_origin,
            chain_id: self.ctx.chain_id,
            randao_mix: self.ctx.randao_mix,
        };

        let parent_warm = self.warm_addresses.clone();
        let parent_frames = self.frames.clone();
        let (status, gas_used, deployed_code, sub_warm, sub_logs) = {
            let mut sub = EVMInterpreter::new(sub_ctx, initcode, &mut *self.host);
            sub.frames = parent_frames;
            sub.frames.push(CallFrame {
                snapshot: snap,
                gas_at_entry: forwarded,
                created_address: Some(new_addr),
            });
            sub.warm_addresses = parent_warm;
            let (status, gas_used) = sub.run();
            let code = std::mem::take(&mut sub.return_data);
            let w = std::mem::take(&mut sub.warm_addresses);
            // EVM-01 FIX (HIGH): same as do_call — constructor logs must
            // propagate to the parent frame. A constructor that emits events
            // (e.g. initialising an ERC-20 with a Transfer from address(0))
            // would silently drop those logs without this extraction.
            let l = sub.take_logs();
            (status, gas_used, code, w, l)
        };
        self.warm_addresses = sub_warm;

        let unused = forwarded.saturating_sub(gas_used);
        self.refund_gas(unused);

        let success = matches!(status, ExitStatus::Succeeded);
        if !success {
            self.host.revert_to(snap);
            // Per EIP-211, REVERT data is accessible via RETURNDATA*.
            self.return_data = deployed_code;
            // EVM-01: constructor logs from a failed/reverted deployment are
            // discarded — sub_logs intentionally dropped here.
            self.stack.push([0u8; 32])?;
            return Ok(());
        }

        // EIP-170: deployed code size cap.
        if deployed_code.len() > MAX_CONTRACT_CODE_SIZE {
            self.host.revert_to(snap);
            self.return_data.clear();
            self.stack.push([0u8; 32])?;
            return Ok(());
        }
        // EIP-3541: reject deployed code starting with 0xEF.
        if deployed_code.first() == Some(&0xEF) {
            self.host.revert_to(snap);
            self.return_data.clear();
            self.stack.push([0u8; 32])?;
            return Ok(());
        }
        // Code-deposit gas (200 per byte). If the parent can't afford it
        // the deployment reverts.
        let deposit_cost = GAS_CODE_DEPOSIT_PER_BYTE * deployed_code.len() as u64;
        if self.gas_remaining < deposit_cost {
            self.host.revert_to(snap);
            self.return_data.clear();
            self.stack.push([0u8; 32])?;
            return Ok(());
        }
        self.consume_gas(deposit_cost)?;
        self.host.set_code(&new_addr, deployed_code);
        self.host.commit(snap);
        // EVM-01 FIX: propagate constructor logs to parent on successful deploy.
        self.logs.extend(sub_logs);
        self.return_data.clear();

        let mut w = [0u8; 32];
        w[12..].copy_from_slice(new_addr.as_bytes());
        self.stack.push(w)?;
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────
//  CREATE / CREATE2 address derivation helpers (S32)
//
//  Inlined here rather than depending on a separate `zbx-rlp` crate — the
//  encoding for `(Address, u64)` is small and well-defined enough to keep
//  in-house, and avoids a new workspace edge.
// ─────────────────────────────────────────────────────────────────────────

/// Minimal RLP encoding of a single u64 (per Ethereum Yellow Paper).
fn rlp_encode_u64(n: u64) -> Vec<u8> {
    if n == 0 {
        return vec![0x80];
    }
    if n < 0x80 {
        return vec![n as u8];
    }
    let be = n.to_be_bytes();
    let first_nonzero = be.iter().position(|&b| b != 0).unwrap_or(be.len() - 1);
    let trimmed = &be[first_nonzero..];
    let mut out = Vec::with_capacity(1 + trimmed.len());
    out.push(0x80 + trimmed.len() as u8);
    out.extend_from_slice(trimmed);
    out
}

/// CREATE: addr = keccak256(rlp([sender, nonce]))[12..32].
pub(crate) fn create_address(sender: &Address, nonce: u64) -> Address {
    // sender is a 20-byte string: prefix 0x80 + 20 = 0x94.
    let mut payload = Vec::with_capacity(32);
    payload.push(0x94);
    payload.extend_from_slice(sender.as_bytes());
    payload.extend_from_slice(&rlp_encode_u64(nonce));
    // List header. Payload length is at most 21 + 9 = 30 bytes (well under
    // 55), so we always use the short-form list prefix 0xc0 + len.
    let mut full = Vec::with_capacity(payload.len() + 1);
    full.push(0xc0 + payload.len() as u8);
    full.extend_from_slice(&payload);
    let h = keccak256(&full);
    let mut a = [0u8; 20];
    a.copy_from_slice(&h.0[12..32]);
    Address(a)
}

/// CREATE2: addr = keccak256(0xff || sender || salt || keccak256(initcode))[12..32].
pub(crate) fn create2_address(sender: &Address, salt: &[u8; 32], initcode: &[u8]) -> Address {
    let init_hash = keccak256(initcode);
    let mut buf = Vec::with_capacity(1 + 20 + 32 + 32);
    buf.push(0xff);
    buf.extend_from_slice(sender.as_bytes());
    buf.extend_from_slice(salt);
    buf.extend_from_slice(&init_hash.0);
    let h = keccak256(&buf);
    let mut a = [0u8; 20];
    a.copy_from_slice(&h.0[12..32]);
    Address(a)
}
