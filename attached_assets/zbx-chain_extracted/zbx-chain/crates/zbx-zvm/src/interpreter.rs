//! ZVM interpreter — executes bytecode opcode by opcode.
//!
//! C53-02 FIX (HIGH): CALL / CALLCODE / DELEGATECALL / STATICCALL / CREATE /
//! CREATE2 / SELFDESTRUCT are now implemented via recursive sub-interpreters
//! that reuse the parent frame's host via a mutable reborrow.  Previously all
//! these opcodes fell through to the `_` arm and halted the frame as
//! InvalidOpcode, making cross-contract calls impossible inside the ZVM.

use crate::{
    opcodes::Opcode,
    stack::ZvmStack,
    memory::ZvmMemory,
    gas::{evm_gas_cost, zvm_gas_cost,
        memory_gas_delta, COLD_SLOAD_COST, WARM_SLOAD_COST,
        COLD_ACCOUNT_COST, WARM_ACCOUNT_COST, SSTORE_COLD_DELTA,
        exp_dynamic_gas, keccak256_dynamic_gas, copy_dynamic_gas, log_dynamic_gas},
    context::{ZvmContext, ZvmResult, ExecutionStatus, ZvmLog, ZvmStructuredLog},
    host::ZvmHost,
    error::ZvmError,
};
use std::collections::HashSet;
use tracing::{debug, trace};
use sha3::{Digest, Keccak256};
use primitive_types::U256;

/// SEC-2026-05-09 Pass-15 (HIGH-Z02 EIP-6780): per-tx record of
/// addresses created inside the current transaction. SELFDESTRUCT
/// only fully deletes an account when its address is in this set;
/// otherwise it just sweeps the balance to the beneficiary. Cleared
/// at every tx boundary by the executor.
pub type CreatedInTx = HashSet<[u8; 20]>;

/// Distinguishes the four CALL-family opcodes for `do_call`.
#[derive(Clone, Copy, Debug)]
enum ZvmCallKind {
    Call,
    CallCode,
    DelegateCall,
    StaticCall,
}

/// Maximum call-stack depth (mirrors EVM Yellow Paper §9.2).
const CALL_DEPTH_LIMIT: usize = 1024;

/// ZVM bytecode interpreter.
pub struct ZvmInterpreter<'a, H: ZvmHost> {
    ctx:    &'a ZvmContext,
    host:   &'a mut H,
    stack:  ZvmStack,
    memory: ZvmMemory,
    pc:     usize,
    gas:    u64,
    logs:   Vec<ZvmLog>,
    zvm_logs: Vec<ZvmStructuredLog>,
    return_data: Vec<u8>,
    log_index: u32,
    /// Current call depth (incremented before each sub-call).
    depth: usize,
    /// SEC-2026-05-09 Pass-15 (HIGH-Z01 EIP-2929): warm address /
    /// slot tracking. Pre-Pass-15 every storage read/write paid a
    /// flat 100 gas regardless of cold/warm status — a contract
    /// could touch hundreds of unrelated SSTORE slots at the wrong
    /// price. Now: first SLOAD on a (contract,key) charges 2100;
    /// follow-up reads of the same slot charge 100. SAme pattern
    /// for BALANCE/EXTCODESIZE/EXTCODEHASH against `accessed_addresses`.
    /// Sets are pre-warmed in `run()` with tx.from + tx.to + the 9
    /// canonical precompiles + every (addr,key) pair in the EIP-2930
    /// access list (consumed via the executor).
    accessed_addresses: HashSet<[u8; 20]>,
    accessed_slots:     HashSet<([u8; 20], [u8; 32])>,
    /// SEC-2026-05-09 Pass-13 (ZVM-T0-JUMPDEST): valid JUMPDEST positions
    /// in `ctx.bytecode`. Pre-Pass-13 the JUMP/JUMPI guards walked the
    /// raw byte at `code[dest]` and accepted any 0x5B byte even if it
    /// was actually the operand of a PUSH instruction — this is the
    /// classic "arbitrary-jump-into-PUSH-data" exploit. Bitmap is built
    /// once per frame in `run()` by scanning the code and skipping
    /// PUSH-N operands.
    jumpdests: Vec<bool>,
}

impl<'a, H: ZvmHost> ZvmInterpreter<'a, H> {
    pub fn new(ctx: &'a ZvmContext, host: &'a mut H) -> Self {
        ZvmInterpreter {
            ctx,
            host,
            stack:  ZvmStack::new(),
            memory: ZvmMemory::new(),
            pc:     0,
            gas:    ctx.gas_limit,
            logs:   Vec::new(),
            zvm_logs: Vec::new(),
            return_data: Vec::new(),
            log_index: 0,
            depth: 0,
            jumpdests: Vec::new(),
            accessed_addresses: HashSet::new(),
            accessed_slots: HashSet::new(),
        }
    }

    /// SEC-2026-05-09 Pass-13 (ZVM-T0-JUMPDEST): build a `valid_dest[i]`
    /// bitmap by scanning the bytecode once and skipping PUSH operands.
    /// Yellow Paper §9.4.3 mandates this analysis — any 0x5B byte that
    /// falls inside a PUSH-N operand window is NOT a valid jump target.
    fn build_jumpdest_bitmap(code: &[u8]) -> Vec<bool> {
        let mut dests = vec![false; code.len()];
        let mut i = 0usize;
        while i < code.len() {
            let op = code[i];
            if op == 0x5B {
                dests[i] = true;
                i += 1;
            } else if (0x60..=0x7F).contains(&op) {
                // PUSH1..PUSH32 — skip operand bytes (1..32).
                let n = (op - 0x5F) as usize;
                i = i.saturating_add(1 + n);
            } else {
                i += 1;
            }
        }
        dests
    }

    /// SEC-2026-05-09 Pass-15 (HIGH-Z01): unified gas-charge helper.
    /// All EIP-2929 cold-deltas + EIP-150 memory expansion charges
    /// route through here so a single OOG check covers them.
    fn charge_gas(&mut self, amt: u64) -> Result<(), ZvmError> {
        if amt > self.gas {
            self.gas = 0;
            return Err(ZvmError::OutOfGas);
        }
        self.gas -= amt;
        Ok(())
    }

    /// SEC-2026-05-09 Pass-15 (HIGH-Z03 EIP-150): charge memory
    /// expansion before any opcode that touches an offset+len region.
    /// Pre-fix the ZVM had zero memory-expansion gas — a single
    /// MSTORE at offset 1 GB allocated 1 GB of host memory for free.
    fn charge_mem_expansion(&mut self, new_size: usize) -> Result<(), ZvmError> {
        // SEC-2026-05-09 Pass-15 architect-review: zero-length windows
        // (e.g. `RETURN 0 0` / `CALL ... 0 0 0 0`) MUST NOT trigger
        // expansion per EVM spec — guard before any rounding.
        if new_size == 0 { return Ok(()); }
        let old_words = self.memory.words() as u64;
        let new_words = ((new_size as u64) + 31) / 32;
        let delta = memory_gas_delta(old_words, new_words);
        if delta == 0 { return Ok(()); }
        self.charge_gas(delta)
    }

    /// Run the interpreter until STOP/RETURN/REVERT or gas exhaustion.
    pub fn run(&mut self) -> ZvmResult {
        // Lazily build JUMPDEST bitmap (cheap; once per frame).
        if self.jumpdests.len() != self.ctx.bytecode.len() {
            self.jumpdests = Self::build_jumpdest_bitmap(&self.ctx.bytecode);
        }
        let code = &self.ctx.bytecode;

        loop {
            if self.pc >= code.len() {
                break; // Implicit STOP
            }

            let byte = code[self.pc];
            let op = match Opcode::from_u8(byte) {
                Some(op) => op,
                None => {
                    return self.finish(ExecutionStatus::InvalidOpcode(byte));
                }
            };

            trace!(pc = self.pc, op = %op, gas = self.gas, "ZVM step");

            // Charge gas
            let cost = if op.is_zvm_native() {
                zvm_gas_cost(op)
            } else {
                evm_gas_cost(op)
            };

            if self.gas < cost {
                return self.finish(ExecutionStatus::OutOfGas);
            }
            self.gas -= cost;

            match self.dispatch(op) {
                Ok(Some(result)) => return result,
                Ok(None) => {}
                Err(e) => {
                    debug!(error = %e, "ZVM execution error");
                    return self.finish(ExecutionStatus::ZvmError(e.to_string()));
                }
            }
        }

        self.finish(ExecutionStatus::Success)
    }

    /// Dispatch a single opcode. Returns Ok(Some(result)) to stop, Ok(None) to continue.
    fn dispatch(&mut self, op: Opcode) -> Result<Option<ZvmResult>, ZvmError> {
        let code = &self.ctx.bytecode;

        match op {
            Opcode::STOP => return Ok(Some(self.finish(ExecutionStatus::Success))),

            // ── Arithmetic (SEC-2026-05-09 Pass-16 ZVM-U256) ─────────────
            // Pre-Pass-16 ADD/MUL/SUB downcast each 256-bit stack word to
            // u128 via `[16..]`, silently truncating the upper 128 bits.
            // For any operand ≥ 2^128 (UniV3 sqrtPriceX96, RAY math, large
            // total supplies) this was a SILENT consensus break vs Eth
            // mainnet — the ZVM produced a different result than every
            // other EVM. All arithmetic now goes through `primitive_types::
            // U256` (already a workspace dep) using overflowing_* / div /
            // rem / pow per Yellow Paper §H.
            Opcode::ADD => {
                let a = self.stack.pop_u256()?;
                let b = self.stack.pop_u256()?;
                self.stack.push_u256(a.overflowing_add(b).0)?;
            }
            Opcode::MUL => {
                let a = self.stack.pop_u256()?;
                let b = self.stack.pop_u256()?;
                self.stack.push_u256(a.overflowing_mul(b).0)?;
            }
            Opcode::SUB => {
                let a = self.stack.pop_u256()?;
                let b = self.stack.pop_u256()?;
                self.stack.push_u256(a.overflowing_sub(b).0)?;
            }
            Opcode::DIV => {
                let a = self.stack.pop_u256()?;
                let b = self.stack.pop_u256()?;
                let r = if b.is_zero() { U256::zero() } else { a / b };
                self.stack.push_u256(r)?;
            }
            Opcode::SDIV => {
                let a = self.stack.pop_u256()?;
                let b = self.stack.pop_u256()?;
                self.stack.push_u256(sdiv_u256(a, b))?;
            }
            Opcode::MOD => {
                let a = self.stack.pop_u256()?;
                let b = self.stack.pop_u256()?;
                let r = if b.is_zero() { U256::zero() } else { a % b };
                self.stack.push_u256(r)?;
            }
            Opcode::SMOD => {
                let a = self.stack.pop_u256()?;
                let b = self.stack.pop_u256()?;
                self.stack.push_u256(smod_u256(a, b))?;
            }
            Opcode::ADDMOD => {
                let a = self.stack.pop_u256()?;
                let b = self.stack.pop_u256()?;
                let n = self.stack.pop_u256()?;
                let r = if n.is_zero() {
                    U256::zero()
                } else {
                    // (a + b) mod n — promote to U512 to avoid wrap.
                    use primitive_types::U512;
                    let s = U512::from(a) + U512::from(b);
                    let m = s % U512::from(n);
                    let mut buf = [0u8; 64];
                    m.to_big_endian(&mut buf);
                    U256::from_big_endian(&buf[32..])
                };
                self.stack.push_u256(r)?;
            }
            Opcode::MULMOD => {
                let a = self.stack.pop_u256()?;
                let b = self.stack.pop_u256()?;
                let n = self.stack.pop_u256()?;
                let r = if n.is_zero() {
                    U256::zero()
                } else {
                    use primitive_types::U512;
                    let p = U512::from(a) * U512::from(b);
                    let m = p % U512::from(n);
                    let mut buf = [0u8; 64];
                    m.to_big_endian(&mut buf);
                    U256::from_big_endian(&buf[32..])
                };
                self.stack.push_u256(r)?;
            }
            Opcode::EXP => {
                let base = self.stack.pop_u256()?;
                let exp  = self.stack.pop_u256()?;
                // EIP-160 dynamic gas (50 per non-zero exponent byte).
                let mut exp_be = [0u8; 32];
                exp.to_big_endian(&mut exp_be);
                self.charge_gas(exp_dynamic_gas(&exp_be))?;
                self.stack.push_u256(base.overflowing_pow(exp).0)?;
            }
            Opcode::SIGNEXTEND => {
                // SIGNEXTEND(b, x): sign-extend x from byte (b+1).
                let b = self.stack.pop_u256()?;
                let x = self.stack.pop_u256()?;
                let r = if b >= U256::from(31u64) {
                    x
                } else {
                    let bit = (b.low_u64() as usize) * 8 + 7;
                    let mut x_be = [0u8; 32];
                    x.to_big_endian(&mut x_be);
                    let sign_byte = 31 - (bit / 8);
                    let sign      = (x_be[sign_byte] >> 7) & 1 == 1;
                    if sign {
                        for i in 0..sign_byte { x_be[i] = 0xFF; }
                    } else {
                        for i in 0..sign_byte { x_be[i] = 0x00; }
                    }
                    U256::from_big_endian(&x_be)
                };
                self.stack.push_u256(r)?;
            }

            // ── Comparison (SEC-2026-05-09 Pass-16) ──────────────────────
            // LT / GT / SLT / SGT were missing entirely (fell through to
            // InvalidOpcode), bricking every Solidity `<` / `>` / signed
            // comparison and every require() bound check.
            Opcode::LT => {
                let a = self.stack.pop_u256()?;
                let b = self.stack.pop_u256()?;
                self.stack.push_u64(if a < b { 1 } else { 0 })?;
            }
            Opcode::GT => {
                let a = self.stack.pop_u256()?;
                let b = self.stack.pop_u256()?;
                self.stack.push_u64(if a > b { 1 } else { 0 })?;
            }
            Opcode::SLT => {
                let a = self.stack.pop_u256()?;
                let b = self.stack.pop_u256()?;
                self.stack.push_u64(if signed_lt(a, b) { 1 } else { 0 })?;
            }
            Opcode::SGT => {
                let a = self.stack.pop_u256()?;
                let b = self.stack.pop_u256()?;
                self.stack.push_u64(if signed_lt(b, a) { 1 } else { 0 })?;
            }

            // ── Bitwise (SEC-2026-05-09 Pass-16) ─────────────────────────
            Opcode::AND => {
                let a = self.stack.pop()?;
                let b = self.stack.pop()?;
                let mut r = [0u8; 32];
                for i in 0..32 { r[i] = a[i] & b[i]; }
                self.stack.push(r)?;
            }
            Opcode::OR => {
                let a = self.stack.pop()?;
                let b = self.stack.pop()?;
                let mut r = [0u8; 32];
                for i in 0..32 { r[i] = a[i] | b[i]; }
                self.stack.push(r)?;
            }
            Opcode::XOR => {
                let a = self.stack.pop()?;
                let b = self.stack.pop()?;
                let mut r = [0u8; 32];
                for i in 0..32 { r[i] = a[i] ^ b[i]; }
                self.stack.push(r)?;
            }
            Opcode::NOT => {
                let a = self.stack.pop()?;
                let mut r = [0u8; 32];
                for i in 0..32 { r[i] = !a[i]; }
                self.stack.push(r)?;
            }
            Opcode::BYTE => {
                // BYTE(i, x): i-th byte of x (0 = MSB), or 0 if i >= 32.
                let i = self.stack.pop_u256()?;
                let x = self.stack.pop()?;
                let r = if i >= U256::from(32u64) {
                    [0u8; 32]
                } else {
                    let mut w = [0u8; 32];
                    w[31] = x[i.low_u64() as usize];
                    w
                };
                self.stack.push(r)?;
            }
            Opcode::SHL => {
                let shift = self.stack.pop_u256()?;
                let value = self.stack.pop_u256()?;
                let r = if shift >= U256::from(256u64) {
                    U256::zero()
                } else {
                    value << shift.low_u64() as usize
                };
                self.stack.push_u256(r)?;
            }
            Opcode::SHR => {
                let shift = self.stack.pop_u256()?;
                let value = self.stack.pop_u256()?;
                let r = if shift >= U256::from(256u64) {
                    U256::zero()
                } else {
                    value >> shift.low_u64() as usize
                };
                self.stack.push_u256(r)?;
            }
            Opcode::SAR => {
                let shift = self.stack.pop_u256()?;
                let value = self.stack.pop_u256()?;
                self.stack.push_u256(sar_u256(value, shift))?;
            }

            // ── KECCAK256 (SEC-2026-05-09 Pass-16) ───────────────────────
            // Was missing → every CREATE2 salt computation, every Solidity
            // mapping key, every interface-id check failed with InvalidOpcode.
            Opcode::KECCAK256 => {
                let offset = self.stack.pop_u64()? as usize;
                let length = self.stack.pop_u64()? as usize;
                self.charge_gas(keccak256_dynamic_gas(length))?;
                self.charge_mem_expansion(offset.saturating_add(length))?;
                let data = self.memory.read_slice(offset, length).to_vec();
                let h = Keccak256::digest(&data);
                let mut w = [0u8; 32];
                w.copy_from_slice(&h);
                self.stack.push(w)?;
            }

            // ── Calldata (SEC-2026-05-09 Pass-16) ────────────────────────
            // Pre-Pass-16 every contract function dispatch (the 4-byte
            // selector load at offset 0) failed → no Solidity contract
            // could be invoked at all.
            Opcode::CALLDATALOAD => {
                let offset = self.stack.pop_u256()?;
                // EVM semantics: out-of-bounds reads return zero bytes.
                let mut w = [0u8; 32];
                if offset < U256::from(self.ctx.calldata.len()) {
                    let off = offset.low_u64() as usize;
                    let avail = self.ctx.calldata.len() - off;
                    let take = avail.min(32);
                    w[..take].copy_from_slice(&self.ctx.calldata[off..off + take]);
                }
                self.stack.push(w)?;
            }
            Opcode::CALLDATASIZE => {
                self.stack.push_u64(self.ctx.calldata.len() as u64)?;
            }
            Opcode::CALLDATACOPY => {
                let dest_off = self.stack.pop_u64()? as usize;
                let src_off  = self.stack.pop_u64()? as usize;
                let len      = self.stack.pop_u64()? as usize;
                self.charge_gas(copy_dynamic_gas(len))?;
                self.charge_mem_expansion(dest_off.saturating_add(len))?;
                let cd = &self.ctx.calldata;
                let mut buf = vec![0u8; len];
                if src_off < cd.len() {
                    let take = (cd.len() - src_off).min(len);
                    buf[..take].copy_from_slice(&cd[src_off..src_off + take]);
                }
                self.memory.write_slice(dest_off, &buf)?;
            }

            // ── Code (SEC-2026-05-09 Pass-16) ────────────────────────────
            Opcode::CODESIZE => {
                self.stack.push_u64(self.ctx.bytecode.len() as u64)?;
            }
            Opcode::CODECOPY => {
                let dest_off = self.stack.pop_u64()? as usize;
                let src_off  = self.stack.pop_u64()? as usize;
                let len      = self.stack.pop_u64()? as usize;
                self.charge_gas(copy_dynamic_gas(len))?;
                self.charge_mem_expansion(dest_off.saturating_add(len))?;
                let bc = self.ctx.bytecode.clone();
                let mut buf = vec![0u8; len];
                if src_off < bc.len() {
                    let take = (bc.len() - src_off).min(len);
                    buf[..take].copy_from_slice(&bc[src_off..src_off + take]);
                }
                self.memory.write_slice(dest_off, &buf)?;
            }
            Opcode::RETURNDATASIZE => {
                self.stack.push_u64(self.return_data.len() as u64)?;
            }
            Opcode::RETURNDATACOPY => {
                let dest_off = self.stack.pop_u64()? as usize;
                let src_off  = self.stack.pop_u64()? as usize;
                let len      = self.stack.pop_u64()? as usize;
                self.charge_gas(copy_dynamic_gas(len))?;
                self.charge_mem_expansion(dest_off.saturating_add(len))?;
                // EVM semantics: revert if read out of return-data bounds
                // (unlike CALLDATACOPY which zero-pads).
                if src_off.saturating_add(len) > self.return_data.len() {
                    return Err(ZvmError::InvalidInput("RETURNDATACOPY out of bounds".into()));
                }
                let buf = self.return_data[src_off..src_off + len].to_vec();
                self.memory.write_slice(dest_off, &buf)?;
            }

            // ── Block info (SEC-2026-05-09 Pass-16) ──────────────────────
            Opcode::BLOCKHASH => {
                let n = self.stack.pop_u64()?;
                let h = self.host.block_hash(n);
                self.stack.push(h)?;
            }
            Opcode::COINBASE => {
                let cb = self.host.coinbase();
                self.stack.push_address(cb)?;
            }
            Opcode::PREVRANDAO => {
                let r = self.host.prevrandao();
                self.stack.push(r)?;
            }
            Opcode::GASLIMIT => {
                self.stack.push_u64(self.host.block_gas_limit())?;
            }
            Opcode::GASPRICE => {
                self.stack.push_u128(self.host.gas_price())?;
            }
            Opcode::SELFBALANCE => {
                let bal = self.host.balance(&self.ctx.contract);
                self.stack.push_u128(bal)?;
            }
            Opcode::BLOBHASH => {
                let i = self.stack.pop_u64()?;
                let h = self.host.blob_hash(i);
                self.stack.push(h)?;
            }

            // ── Account (SEC-2026-05-09 Pass-16: cold/warm) ──────────────
            // BALANCE / EXTCODESIZE / EXTCODEHASH now bump the EIP-2929
            // accessed_addresses set; first touch pays 2600 instead of 100.
            Opcode::BALANCE => {
                let addr = self.stack.pop_address()?;
                let extra = if self.accessed_addresses.insert(addr) {
                    COLD_ACCOUNT_COST.saturating_sub(WARM_ACCOUNT_COST)
                } else { 0 };
                self.charge_gas(extra)?;
                let bal = self.host.balance(&addr);
                self.stack.push_u128(bal)?;
            }

            // ── Stack: PUSH0 / MSIZE (SEC-2026-05-09 Pass-16) ────────────
            Opcode::PUSH0 => {
                self.stack.push([0u8; 32])?;
            }
            Opcode::MSIZE => {
                // Memory size in BYTES, rounded up to the next word boundary.
                let s = (self.memory.size() + 31) / 32 * 32;
                self.stack.push_u64(s as u64)?;
            }

            // ── Transient storage + MCOPY (Cancun, SEC-2026-05-09 Pass-16)
            // EIP-1153 + EIP-5656. Pre-Pass-16 these were missing → every
            // Cancun reentrancy-guard pattern (OZ TransientReentrancyGuard,
            // UniV4 PoolManager.unlock) bricked.
            Opcode::TLOAD => {
                let key = self.stack.pop()?;
                let v   = self.host.transient_load(&self.ctx.contract, &key);
                self.stack.push(v)?;
            }
            Opcode::TSTORE => {
                if self.ctx.is_static { return Err(ZvmError::StaticStateChange); }
                let key = self.stack.pop()?;
                let val = self.stack.pop()?;
                self.host.transient_store(&self.ctx.contract, key, val);
            }
            Opcode::MCOPY => {
                let dest_off = self.stack.pop_u64()? as usize;
                let src_off  = self.stack.pop_u64()? as usize;
                let len      = self.stack.pop_u64()? as usize;
                self.charge_gas(copy_dynamic_gas(len))?;
                let end = dest_off.max(src_off).saturating_add(len);
                self.charge_mem_expansion(end)?;
                self.memory.copy(dest_off, src_off, len)?;
            }

            // ── Push ─────────────────────────────────────────────────────
            op if (op as u8) >= 0x60 && (op as u8) <= 0x7F => {
                let n = (op as u8 - 0x5F) as usize;
                let start = self.pc + 1;
                let end = start + n;
                if end > code.len() { return Err(ZvmError::UnexpectedEnd); }
                let mut word = [0u8; 32];
                word[32 - n..].copy_from_slice(&code[start..end]);
                self.stack.push(word)?;
                self.pc += n;
            }

            // ── DUP ──────────────────────────────────────────────────────
            op if (op as u8) >= 0x80 && (op as u8) <= 0x8F => {
                let n = (op as u8 - 0x7F) as usize;
                self.stack.dup(n)?;
            }

            // ── SWAP ─────────────────────────────────────────────────────
            op if (op as u8) >= 0x90 && (op as u8) <= 0x9F => {
                let n = (op as u8 - 0x8F) as usize;
                self.stack.swap(n)?;
            }

            // ── Memory ───────────────────────────────────────────────────
            // SEC-2026-05-09 Pass-15 (HIGH-Z03 EIP-150): every memory
            // op charges expansion gas before the touch. Without this
            // a single MSTORE at offset 1 GiB allocated 1 GiB of host
            // memory for free.
            Opcode::MLOAD => {
                let offset = self.stack.pop_u64()? as usize;
                self.charge_mem_expansion(offset.saturating_add(32))?;
                let word = self.memory.load(offset)?;
                self.stack.push(word)?;
            }
            Opcode::MSTORE => {
                let offset = self.stack.pop_u64()? as usize;
                let word = self.stack.pop()?;
                self.charge_mem_expansion(offset.saturating_add(32))?;
                self.memory.store(offset, word)?;
            }
            Opcode::MSTORE8 => {
                let offset = self.stack.pop_u64()? as usize;
                let byte  = self.stack.pop()?[31];
                self.charge_mem_expansion(offset.saturating_add(1))?;
                self.memory.store8(offset, byte)?;
            }

            // ── Block info ───────────────────────────────────────────────
            Opcode::NUMBER => self.stack.push_u64(self.ctx.block_number)?,
            Opcode::TIMESTAMP => self.stack.push_u64(self.ctx.block_timestamp)?,
            Opcode::CHAINID => self.stack.push_u64(self.ctx.chain_id)?,
            Opcode::BASEFEE => self.stack.push_u128(self.ctx.base_fee)?,
            Opcode::BLOBBASEFEE => self.stack.push_u128(self.ctx.blob_base_fee)?,

            // ── Call context ─────────────────────────────────────────────
            Opcode::CALLER => self.stack.push_address(self.ctx.caller)?,
            Opcode::ADDRESS => self.stack.push_address(self.ctx.contract)?,
            Opcode::CALLVALUE => self.stack.push_u128(self.ctx.value)?,
            Opcode::GAS => self.stack.push_u64(self.gas)?,
            // SEC-2026-05-09 Pass-13 (ZVM-T0-ORIGIN): pre-Pass-13 ORIGIN
            // aliased to CALLER, breaking EIP-3 semantics + every
            // `tx.origin == msg.sender` EOA-only guard. Now reads the
            // dedicated ctx.origin which is propagated unchanged across
            // every sub-call (see do_call / do_create below).
            Opcode::ORIGIN => self.stack.push_address(self.ctx.origin)?,

            // ── Storage ──────────────────────────────────────────────────
            // SEC-2026-05-09 Pass-15 (HIGH-Z01 EIP-2929): cold/warm
            // accounting on every (contract,slot) access. The flat-100
            // pricing pre-fix made cold-state DoS economically free.
            Opcode::SLOAD => {
                let key = self.stack.pop()?;
                let slot_id = (self.ctx.contract, key);
                let extra = if self.accessed_slots.insert(slot_id) {
                    // Cold: already paid the 100 base in the dispatcher;
                    // top up to 2100.
                    COLD_SLOAD_COST.saturating_sub(WARM_SLOAD_COST)
                } else { 0 };
                self.charge_gas(extra)?;
                let val = self.host.storage_load(&self.ctx.contract, &key);
                self.stack.push(val)?;
            }
            Opcode::SSTORE => {
                if self.ctx.is_static { return Err(ZvmError::StaticStateChange); }
                let key = self.stack.pop()?;
                let val = self.stack.pop()?;
                let slot_id = (self.ctx.contract, key);
                let extra = if self.accessed_slots.insert(slot_id) {
                    SSTORE_COLD_DELTA
                } else { 0 };
                self.charge_gas(extra)?;
                self.host.storage_store(&self.ctx.contract, key, val);
            }

            // ── Control flow ─────────────────────────────────────────────
            // SEC-2026-05-09 Pass-13 (ZVM-T0-JUMPDEST): validate
            // against the precomputed bitmap, NOT the raw byte. A 0x5B
            // byte sitting in a PUSH operand window is no longer a
            // valid target (Yellow Paper §9.4.3).
            Opcode::JUMP => {
                let dest = self.stack.pop_u64()? as usize;
                if dest >= code.len() || !self.jumpdests.get(dest).copied().unwrap_or(false) {
                    return Err(ZvmError::InvalidJump(dest));
                }
                self.pc = dest;
                return Ok(None);
            }
            Opcode::JUMPI => {
                let dest = self.stack.pop_u64()? as usize;
                let cond = self.stack.pop()?;
                if cond != [0u8; 32] {
                    if dest >= code.len() || !self.jumpdests.get(dest).copied().unwrap_or(false) {
                        return Err(ZvmError::InvalidJump(dest));
                    }
                    self.pc = dest;
                    return Ok(None);
                }
            }
            Opcode::JUMPDEST => {}
            Opcode::PC => self.stack.push_u64(self.pc as u64)?,
            Opcode::POP => { self.stack.pop()?; }

            // ── Comparison ───────────────────────────────────────────────
            Opcode::EQ => {
                let a = self.stack.pop()?;
                let b = self.stack.pop()?;
                self.stack.push_u64(if a == b { 1 } else { 0 })?;
            }
            Opcode::ISZERO => {
                let a = self.stack.pop()?;
                self.stack.push_u64(if a == [0u8; 32] { 1 } else { 0 })?;
            }

            // ── Return / Revert ──────────────────────────────────────────
            // SEC-2026-05-09 Pass-15 (HIGH-Z03 EIP-150): charge memory
            // expansion gas before reading the return-data window.
            Opcode::RETURN => {
                let offset = self.stack.pop_u64()? as usize;
                let len    = self.stack.pop_u64()? as usize;
                self.charge_mem_expansion(offset.saturating_add(len))?;
                self.return_data = self.memory.read_slice(offset, len).to_vec();
                return Ok(Some(self.finish(ExecutionStatus::Success)));
            }
            Opcode::REVERT => {
                let offset = self.stack.pop_u64()? as usize;
                let len    = self.stack.pop_u64()? as usize;
                self.charge_mem_expansion(offset.saturating_add(len))?;
                self.return_data = self.memory.read_slice(offset, len).to_vec();
                return Ok(Some(self.finish(ExecutionStatus::Revert)));
            }
            Opcode::INVALID => return Ok(Some(self.finish(ExecutionStatus::InvalidOpcode(0xFE)))),

            // ── CALL family (C53-02) ─────────────────────────────────────
            Opcode::CALL         => self.do_call(ZvmCallKind::Call)?,
            Opcode::CALLCODE     => self.do_call(ZvmCallKind::CallCode)?,
            Opcode::DELEGATECALL => self.do_call(ZvmCallKind::DelegateCall)?,
            Opcode::STATICCALL   => self.do_call(ZvmCallKind::StaticCall)?,

            // ── CREATE family (C53-02) ───────────────────────────────────
            Opcode::CREATE  => self.do_create(false)?,
            Opcode::CREATE2 => self.do_create(true)?,

            // ── SELFDESTRUCT (C53-02 + Pass-15 HIGH-Z02 EIP-6780) ────────
            // EIP-6780: SELFDESTRUCT only fully destroys an account if
            // it was created in the SAME transaction. Otherwise it
            // simply sweeps the balance to the beneficiary and returns
            // success — but the contract code, storage, and account
            // remain. Pre-fix the ZVM treated SELFDESTRUCT as an
            // unconditional sweep (which silently behaves correctly
            // because we never actually delete the contract — but the
            // semantic intent was wrong, and a future host that did
            // honour deletion would diverge from Cancun behaviour).
            // The host now decides via `selfdestruct(contract, beneficiary,
            // created_this_tx)`; if the implementation doesn't have
            // tx-creation tracking yet it should treat every account
            // as not-created-this-tx (= sweep only) which is the
            // safe default.
            Opcode::SELFDESTRUCT => {
                if self.ctx.is_static { return Err(ZvmError::StaticStateChange); }
                let beneficiary = self.stack.pop_address()?;
                // Task #8 (EIP-6780): host owns the (sweep + enqueue)
                // policy. Default impl sweeps balance only; production
                // host also records the (contract, beneficiary) pair
                // so the executor can apply full deletion at end-of-tx
                // iff `was_created_this_tx(contract)`.
                self.host.selfdestruct(&self.ctx.contract, &beneficiary);
                return Ok(Some(self.finish(ExecutionStatus::Success)));
            }

            // ── ZVM Native Opcodes ────────────────────────────────────────

            Opcode::PAYID => {
                let ptr = self.stack.pop_u64()? as usize;
                let len = self.stack.pop_u64()? as usize;
                let pay_id = self.memory.read_string(ptr, len)?;
                let addr = self.host.resolve_pay_id(&pay_id).unwrap_or([0u8; 20]);
                self.stack.push_address(addr)?;
            }

            Opcode::ZUSDBAL => {
                let addr = self.stack.pop_address()?;
                let bal = self.host.zusd_balance(&addr);
                self.stack.push_u128(bal)?;
            }

            Opcode::ZBXPRICE => {
                let price = self.host.zbx_price_usd();
                self.stack.push_u128(price)?;
            }

            Opcode::ZBXTIME => {
                self.stack.push_u64(5000)?;
            }

            Opcode::AASENDER => {
                let sender = self.ctx.aa_sender.unwrap_or(self.ctx.caller);
                self.stack.push_address(sender)?;
            }

            Opcode::CHAINVER => {
                self.stack.push_u64(crate::ZVM_VERSION as u64)?;
            }

            Opcode::BLOBFEE => {
                let fee = self.host.blob_base_fee();
                self.stack.push_u128(fee)?;
            }

            Opcode::PAYIDSET => {
                let addr = self.stack.pop_address()?;
                let has = self.host.has_pay_id(&addr);
                self.stack.push_u64(if has { 1 } else { 0 })?;
            }

            Opcode::ZBXBURN => {
                if self.ctx.is_static { return Err(ZvmError::StaticStateChange); }
                let amount = self.stack.pop_u128()?;
                self.host.burn_zbx(&self.ctx.caller, amount)?;
            }

            // ── EVM EXTCODE* ─────────────────────────────────────────
            // SEC-2026-05-09 Pass-10: prior versions sent these to the
            // catch-all `_` arm and halted as InvalidOpcode, so any
            // contract that introspected another address (ERC-165 checks,
            // proxies, factory deploy verification) bricked.
            Opcode::EXTCODESIZE => {
                let addr = self.stack.pop_address()?;
                // SEC-2026-05-09 Pass-16 EIP-2929 cold/warm.
                let extra = if self.accessed_addresses.insert(addr) {
                    COLD_ACCOUNT_COST.saturating_sub(WARM_ACCOUNT_COST)
                } else { 0 };
                self.charge_gas(extra)?;
                let sz   = self.host.code_size(&addr) as u64;
                self.stack.push_u64(sz)?;
            }

            Opcode::EXTCODEHASH => {
                let addr = self.stack.pop_address()?;
                let extra = if self.accessed_addresses.insert(addr) {
                    COLD_ACCOUNT_COST.saturating_sub(WARM_ACCOUNT_COST)
                } else { 0 };
                self.charge_gas(extra)?;
                let h    = self.host.code_hash(&addr);
                self.stack.push(h)?;
            }

            Opcode::EXTCODECOPY => {
                let addr        = self.stack.pop_address()?;
                let dest_off    = self.stack.pop_u64()? as usize;
                let code_off    = self.stack.pop_u64()? as usize;
                let len         = self.stack.pop_u64()? as usize;
                // SEC-2026-05-09 Pass-16: cold-account + copy + mem-expansion gas.
                let extra = if self.accessed_addresses.insert(addr) {
                    COLD_ACCOUNT_COST.saturating_sub(WARM_ACCOUNT_COST)
                } else { 0 };
                self.charge_gas(extra)?;
                self.charge_gas(copy_dynamic_gas(len))?;
                self.charge_mem_expansion(dest_off.saturating_add(len))?;
                let code        = self.host.code(&addr);
                // Yellow Paper §H.2: out-of-bounds reads past code length
                // return zero bytes (callers rely on this for safe pad).
                let mut buf = vec![0u8; len];
                if code_off < code.len() {
                    let take = (code.len() - code_off).min(len);
                    buf[..take].copy_from_slice(&code[code_off..code_off + take]);
                }
                self.memory.write_slice(dest_off, &buf)?;
            }

            // ── EVM LOG0–LOG4 ────────────────────────────────────────
            // SEC-2026-05-09 Pass-10: previously fell through to `_` and
            // halted the frame, which broke every ERC-20/721 Transfer
            // emit and made indexers unable to follow the chain.
            //
            // Stack pop order (per Yellow Paper §H.2): offset, length, topic1..topicN
            Opcode::LOG0 | Opcode::LOG1 | Opcode::LOG2 | Opcode::LOG3 | Opcode::LOG4 => {
                if self.ctx.is_static {
                    return Err(ZvmError::StaticStateChange);
                }
                let n_topics = (op as u8 - Opcode::LOG0 as u8) as usize;
                let offset   = self.stack.pop_u64()? as usize;
                let length   = self.stack.pop_u64()? as usize;
                // SEC-2026-05-09 Pass-16: dynamic gas (8/byte data + 375/topic)
                // and mem-expansion before the read. Pre-Pass-16 a contract
                // could LOG4 with 1 GiB data window for the flat 375 base.
                self.charge_gas(log_dynamic_gas(n_topics as u8, length))?;
                self.charge_mem_expansion(offset.saturating_add(length))?;
                let mut topics = Vec::with_capacity(n_topics);
                for _ in 0..n_topics {
                    topics.push(self.stack.pop()?);
                }
                let data = self.memory.read_slice(offset, length).to_vec();
                let log = ZvmLog {
                    address: self.ctx.contract,
                    topics:  topics.clone(),
                    data:    data.clone(),
                };
                self.logs.push(log);
                self.host.emit_log(&self.ctx.contract, topics, data);
            }

            Opcode::ZVMLOG => {
                let key_ptr = self.stack.pop_u64()? as usize;
                let key_len = self.stack.pop_u64()? as usize;
                let val_ptr = self.stack.pop_u64()? as usize;
                let val_len = self.stack.pop_u64()? as usize;
                let key = self.memory.read_string(key_ptr, key_len)?;
                let val = self.memory.read_string(val_ptr, val_len)?;
                self.host.emit_zvm_log(&key, &val);
                self.zvm_logs.push(ZvmStructuredLog {
                    key, value: val,
                    block: self.ctx.block_number,
                    index: self.log_index,
                });
                self.log_index += 1;
            }

            _ => {
                // Any remaining unimplemented opcode (BLOCKHASH, etc.) halts
                // the frame — same as EVM Yellow Paper §H.
                // Pass-10: LOG0-4, EXTCODESIZE/COPY/HASH now have real
                // arms above; do not re-add them to this comment list.
                let raw_byte = op as u8;
                debug!(
                    op = %op,
                    byte = format!("0x{:02x}", raw_byte),
                    "ZVM: unimplemented opcode — halting frame as INVALID"
                );
                return Ok(Some(self.finish(ExecutionStatus::InvalidOpcode(raw_byte))));
            }
        }

        self.pc += 1;
        Ok(None)
    }

    // ─────────────────────────────────────────────────────────────────────
    //  CALL family dispatcher (C53-02)
    // ─────────────────────────────────────────────────────────────────────

    /// Common handler for CALL / CALLCODE / DELEGATECALL / STATICCALL.
    ///
    /// Stack pop order:
    ///   CALL / CALLCODE   : gas, addr, value, argsOff, argsLen, retOff, retLen
    ///   DELEGATECALL / STATICCALL: gas, addr, argsOff, argsLen, retOff, retLen
    fn do_call(&mut self, kind: ZvmCallKind) -> Result<(), ZvmError> {
        let gas_req  = self.stack.pop_u64()?;
        let target   = self.stack.pop_address()?;
        let value    = match kind {
            ZvmCallKind::Call | ZvmCallKind::CallCode => self.stack.pop_u128()?,
            ZvmCallKind::DelegateCall | ZvmCallKind::StaticCall => 0u128,
        };

        // Static-frame guard.
        if matches!(kind, ZvmCallKind::Call | ZvmCallKind::CallCode)
            && self.ctx.is_static
            && value > 0
        {
            return Err(ZvmError::StaticStateChange);
        }

        let args_off = self.stack.pop_u64()? as usize;
        let args_len = self.stack.pop_u64()? as usize;
        let ret_off  = self.stack.pop_u64()? as usize;
        let ret_len  = self.stack.pop_u64()? as usize;

        // SEC-2026-05-09 Pass-16: cold-account bump on first touch of CALL
        // target + mem-expansion gas across BOTH the args and return windows.
        // Pre-Pass-16 every cross-contract call paid the flat 100 dispatcher
        // base regardless of cold/warm + a contract could DoS via giant
        // ret_len with no allocation cost.
        let extra = if self.accessed_addresses.insert(target) {
            COLD_ACCOUNT_COST.saturating_sub(WARM_ACCOUNT_COST)
        } else { 0 };
        self.charge_gas(extra)?;
        self.charge_mem_expansion(args_off.saturating_add(args_len))?;
        self.charge_mem_expansion(ret_off.saturating_add(ret_len))?;

        // Depth guard.
        if self.depth >= CALL_DEPTH_LIMIT {
            self.return_data.clear();
            self.stack.push_u64(0)?;
            return Ok(());
        }

        // Read calldata from memory (owned copy, releases borrow).
        let calldata = self.memory.read_slice(args_off, args_len).to_vec();

        // SEC-2026-05-09 Pass-13 (ZVM-T0-PRECOMPILE): if target is a
        // precompile address (0x01..=0x0F), dispatch to the registered
        // precompile handler INSTEAD of executing whatever bytecode
        // sits at that account. Pre-Pass-13 the sub-frame ran an empty
        // `host.code(&precompile)` → no-op success, which silently
        // returned 32 zero bytes for ecrecover (every signature
        // recovery succeeded with `address(0)` → trivial auth bypass)
        // and broke every other precompile-dependent contract.
        if target[..19].iter().all(|&b| b == 0) && (1..=0x0F).contains(&target[19]) {
            return self.do_precompile_call(&target, &calldata, gas_req, ret_off, ret_len);
        }

        // Cross-VM CALL gate. The host rejects calls that cross the
        // EVM/ZVM boundary; on rejection we skip sub-execution entirely
        // and push 0 (failure) — NOT 1 — distinct from a successful
        // CALL into an empty contract.
        if !self.host.is_call_allowed(&target) {
            self.return_data.clear();
            self.stack.push_u64(0)?;
            return Ok(());
        }

        // Read target code (owned, releases immutable borrow of host).
        let code = self.host.code(&target);

        // Determine caller/contract for the sub-frame.
        let (sub_caller, sub_contract) = match kind {
            ZvmCallKind::DelegateCall => (self.ctx.caller, self.ctx.contract),
            _                         => (self.ctx.contract, target),
        };
        let sub_static = self.ctx.is_static || matches!(kind, ZvmCallKind::StaticCall);

        // Value transfer before sub-call (CALL only).
        if matches!(kind, ZvmCallKind::Call) && value > 0 {
            if self.host.balance(&self.ctx.contract) < value {
                self.return_data.clear();
                self.stack.push_u64(0)?;
                return Ok(());
            }
            self.host.transfer(&self.ctx.contract, &target, value)?;
        }

        // Forward gas using the 63/64 rule.
        let forwarded = gas_req.min((self.gas / 64) * 63);
        self.gas = self.gas.saturating_sub(forwarded);

        let sub_ctx = ZvmContext {
            bytecode:       code,
            calldata,
            caller:         sub_caller,
            contract:       sub_contract,
            value:          if matches!(kind, ZvmCallKind::DelegateCall) { self.ctx.value } else { value },
            gas_limit:      forwarded,
            is_static:      sub_static,
            block_number:   self.ctx.block_number,
            block_timestamp: self.ctx.block_timestamp,
            base_fee:       self.ctx.base_fee,
            blob_base_fee:  self.ctx.blob_base_fee,
            chain_id:       self.ctx.chain_id,
            aa_sender:      None,
            zbx_price_usd:  self.ctx.zbx_price_usd,
            // Pass-13 (ZVM-T0-ORIGIN): tx.origin is preserved at every depth.
            origin:         self.ctx.origin,
        };

        // Run sub-interpreter using a mutable reborrow of the host.
        // The explicit struct literal lets Rust infer a shorter lifetime
        // for the sub-interpreter, allowing the reborrow to end when
        // the block exits and the parent frame continues.
        // SEC-2026-05-09 Pass-15 architect-review: merge sub-frame's
        // accessed_* sets back into the parent so EIP-2929 warm pricing
        // is tx-global (per spec), not frame-local.
        let (sub_result, sub_warm_addrs, sub_warm_slots) = {
            let mut sub = ZvmInterpreter {
                ctx:         &sub_ctx,
                host:        &mut *self.host,
                stack:       ZvmStack::new(),
                memory:      ZvmMemory::new(),
                pc:          0,
                gas:         forwarded,
                logs:        Vec::new(),
                zvm_logs:    Vec::new(),
                return_data: Vec::new(),
                log_index:   0,
                depth:       self.depth + 1,
                jumpdests:   Vec::new(),
                accessed_addresses: self.accessed_addresses.clone(),
                accessed_slots:     self.accessed_slots.clone(),
            };
            let r = sub.run();
            (r, sub.accessed_addresses, sub.accessed_slots)
        };
        self.accessed_addresses.extend(sub_warm_addrs);
        self.accessed_slots.extend(sub_warm_slots);

        // Refund unused gas to parent.
        self.gas = self.gas.saturating_add(sub_result.gas_remaining);
        self.return_data = sub_result.return_data.clone();

        // Copy return data into parent memory.
        let copy_len = ret_len.min(sub_result.return_data.len());
        for i in 0..copy_len {
            self.memory.store8(ret_off + i, sub_result.return_data[i])?;
        }

        let success = matches!(sub_result.status, ExecutionStatus::Success);
        self.stack.push_u64(if success { 1 } else { 0 })?;
        Ok(())
    }

    // ─────────────────────────────────────────────────────────────────────
    //  CREATE / CREATE2 dispatcher (C53-02)
    // ─────────────────────────────────────────────────────────────────────

    /// Common handler for CREATE (`with_salt = false`) and CREATE2.
    ///
    /// Stack pop order:
    ///   CREATE  : value, offset, length
    ///   CREATE2 : value, offset, length, salt
    fn do_create(&mut self, with_salt: bool) -> Result<(), ZvmError> {
        if self.ctx.is_static {
            return Err(ZvmError::StaticStateChange);
        }

        let value  = self.stack.pop_u128()?;
        let off    = self.stack.pop_u64()? as usize;
        let len    = self.stack.pop_u64()? as usize;
        let salt   = if with_salt { Some(self.stack.pop()?) } else { None };

        // ZVM-02 FIX (MEDIUM): EIP-3860 (Shanghai) initcode size limit.
        // Max initcode = 2 × MAX_CODE_SIZE = 49 152 bytes. Pre-fix CREATE /
        // CREATE2 accepted unbounded initcode for the flat 32 000-gas base
        // cost — an OOM DoS vector (gigabyte initcode at 32 k gas).
        const MAX_INITCODE_SIZE: usize = 2 * 24_576; // 49 152
        if len > MAX_INITCODE_SIZE {
            self.return_data.clear();
            self.stack.push([0u8; 32])?;
            return Ok(());
        }

        // ZVM-02 FIX (MEDIUM): EIP-3860 per-word initcode charge: 2 gas per
        // 32-byte word. Must be charged even if depth/balance checks abort,
        // so we charge it here after the stack is fully consumed.
        let initcode_word_cost = 2u64.saturating_mul((len as u64 + 31) / 32);
        self.charge_gas(initcode_word_cost)?;

        // ZVM-03 FIX (MEDIUM): CREATE2 must additionally pay the keccak256
        // hashing cost for the initcode (used to derive the new address).
        // Pre-fix CREATE2 paid 0 gas for the keccak256(initcode) call.
        if with_salt {
            self.charge_gas(keccak256_dynamic_gas(len))?;
        }

        // ZVM-03 FIX (MEDIUM): charge memory expansion for the initcode
        // window before reading it. Pre-fix do_create read directly from
        // memory without any expansion charge, allowing free allocation.
        self.charge_mem_expansion(off.saturating_add(len))?;

        // Depth guard.
        if self.depth >= CALL_DEPTH_LIMIT {
            self.return_data.clear();
            self.stack.push([0u8; 32])?;
            return Ok(());
        }

        // Balance check.
        if self.host.balance(&self.ctx.contract) < value {
            self.stack.push([0u8; 32])?;
            return Ok(());
        }

        // Read initcode (owned copy, releases memory borrow).
        let initcode = self.memory.read_slice(off, len).to_vec();

        // Compute new contract address.
        let creator_nonce = self.host.nonce(&self.ctx.contract);
        let new_addr = if let Some(s) = salt {
            create2_address(&self.ctx.contract, &s, &initcode)
        } else {
            create_address(&self.ctx.contract, creator_nonce)
        };

        // Bump nonce before sub-call.
        self.host.inc_nonce(&self.ctx.contract);

        // Collision check.
        if !self.host.code(&new_addr).is_empty() || self.host.nonce(&new_addr) > 0 {
            self.stack.push([0u8; 32])?;
            return Ok(());
        }

        // Value transfer.
        if value > 0 {
            self.host.transfer(&self.ctx.contract, &new_addr, value)?;
        }
        // EIP-161: new account nonce = 1.
        self.host.inc_nonce(&new_addr);

        // Forward gas (63/64 rule).
        let forwarded = (self.gas / 64) * 63;
        self.gas = self.gas.saturating_sub(forwarded);

        let sub_ctx = ZvmContext {
            bytecode:       initcode,
            calldata:       vec![],
            caller:         self.ctx.contract,
            contract:       new_addr,
            value,
            gas_limit:      forwarded,
            is_static:      false,
            block_number:   self.ctx.block_number,
            block_timestamp: self.ctx.block_timestamp,
            base_fee:       self.ctx.base_fee,
            blob_base_fee:  self.ctx.blob_base_fee,
            chain_id:       self.ctx.chain_id,
            aa_sender:      None,
            zbx_price_usd:  self.ctx.zbx_price_usd,
            // Pass-13 (ZVM-T0-ORIGIN): tx.origin survives CREATE.
            origin:         self.ctx.origin,
        };

        let sub_result = {
            let mut sub = ZvmInterpreter {
                ctx:         &sub_ctx,
                host:        &mut *self.host,
                stack:       ZvmStack::new(),
                memory:      ZvmMemory::new(),
                pc:          0,
                gas:         forwarded,
                logs:        Vec::new(),
                zvm_logs:    Vec::new(),
                return_data: Vec::new(),
                log_index:   0,
                depth:       self.depth + 1,
                jumpdests:   Vec::new(),
                accessed_addresses: self.accessed_addresses.clone(),
                accessed_slots:     self.accessed_slots.clone(),
            };
            let r = sub.run();
            (r, sub.accessed_addresses, sub.accessed_slots)
        };
        // SEC-2026-05-09 Pass-15 architect-review: merge sub-frame
        // warm sets back into parent (tx-global EIP-2929).
        let (sub_result, sub_warm_addrs, sub_warm_slots) = sub_result;
        self.accessed_addresses.extend(sub_warm_addrs);
        self.accessed_slots.extend(sub_warm_slots);

        self.gas = self.gas.saturating_add(sub_result.gas_remaining);

        let success = matches!(sub_result.status, ExecutionStatus::Success);
        if !success {
            self.return_data = sub_result.return_data;
            self.stack.push([0u8; 32])?;
            return Ok(());
        }

        // Deploy the bytecode returned by the initcode execution.
        let deployed = sub_result.return_data;

        // EIP-170: max deployed code size = 24576 bytes.
        if deployed.len() > 24_576 {
            self.stack.push([0u8; 32])?;
            return Ok(());
        }
        // EIP-3541: reject 0xEF prefix (EOF marker).
        if deployed.first() == Some(&0xEF) {
            self.stack.push([0u8; 32])?;
            return Ok(());
        }
        // Code-deposit gas: 200 per byte.
        let deposit_cost = 200u64 * deployed.len() as u64;
        if self.gas < deposit_cost {
            self.stack.push([0u8; 32])?;
            return Ok(());
        }
        self.gas -= deposit_cost;

        self.host.set_code(&new_addr, deployed);
        // Task #8 (EIP-6780): record the new account in the per-tx
        // creation set so a subsequent SELFDESTRUCT on `new_addr`
        // within the SAME tx is upgraded from sweep-only to full
        // deletion at end-of-tx.
        self.host.mark_created_this_tx(&new_addr);
        self.return_data.clear();

        let mut w = [0u8; 32];
        w[12..].copy_from_slice(&new_addr);
        self.stack.push(w)?;
        Ok(())
    }

    /// SEC-2026-05-09 Pass-13 (ZVM-T0-PRECOMPILE): execute a precompile
    /// call (target address 0x01..=0x0F). Charges the precompile's
    /// declared gas cost out of the parent frame's remaining gas (no
    /// 63/64 forwarding — precompiles run in the parent's accounting),
    /// copies output to parent memory under `ret_off` / `ret_len`, and
    /// pushes 1 (success) or 0 (failure) to the parent stack.
    fn do_precompile_call(
        &mut self,
        target: &[u8; 20],
        calldata: &[u8],
        gas_req: u64,
        ret_off: usize,
        ret_len: usize,
    ) -> Result<(), ZvmError> {
        let avail = gas_req.min(self.gas);

        // Task #3 (Precompile 0x0A — PayID resolution): stateful precompile.
        // Cannot go through the stateless `call_precompile` dispatcher
        // because it reads chain state via the host. Adapt the host into
        // a `PayIdLookup` for the duration of the call.
        if target[19] == 0x0A {
            struct HostAdapter<'a, H: ZvmHost + ?Sized>(&'a H);
            impl<H: ZvmHost + ?Sized> crate::precompiles::PayIdLookup for HostAdapter<'_, H> {
                fn resolve(&self, name: &[u8]) -> Option<[u8; 20]> {
                    self.0.resolve_pay_id_bytes(name)
                }
                fn reverse(&self, addr: &[u8; 20]) -> Option<Vec<u8>> {
                    self.0.reverse_pay_id(addr)
                }
            }
            let adapter = HostAdapter(&*self.host);
            match crate::precompiles::payid_resolve_with(calldata, avail, &adapter) {
                Ok((output, gas_used)) => {
                    self.gas = self.gas.saturating_sub(gas_used);
                    let copy_len = ret_len.min(output.len());
                    for i in 0..copy_len {
                        self.memory.store8(ret_off + i, output[i])?;
                    }
                    self.return_data = output;
                    self.stack.push_u64(1)?;
                }
                Err(_) => {
                    self.return_data.clear();
                    self.stack.push_u64(0)?;
                }
            }
            return Ok(());
        }

        // Task #5 (Precompile 0x0C — Price oracle read): stateful precompile
        // routed through a host adapter that bridges `storage_load` to
        // `OracleStateReader::read_slot`. Keeps EVM and ZVM byte-identical.
        if target[19] == 0x0C {
            struct OracleAdapter<'a, H: ZvmHost + ?Sized>(&'a H);
            impl<H: ZvmHost + ?Sized> zbx_crypto::oracle_state::OracleStateReader
                for OracleAdapter<'_, H>
            {
                fn read_slot(&self, addr: &[u8; 20], slot: &[u8; 32]) -> [u8; 32] {
                    self.0.storage_load(addr, slot)
                }
            }
            let adapter = OracleAdapter(&*self.host);
            match crate::precompiles::price_oracle_with(calldata, avail, &adapter) {
                Ok((output, gas_used)) => {
                    self.gas = self.gas.saturating_sub(gas_used);
                    let copy_len = ret_len.min(output.len());
                    for i in 0..copy_len {
                        self.memory.store8(ret_off + i, output[i])?;
                    }
                    self.return_data = output;
                    self.stack.push_u64(1)?;
                }
                Err(_) => {
                    self.return_data.clear();
                    self.stack.push_u64(0)?;
                }
            }
            return Ok(());
        }

        // Task #7 (Precompile 0x0F — ZUSD vault state direct-read): stateful
        // precompile routed through a host adapter that bridges
        // `ZvmHost::storage_load` to `OracleStateReader::read_slot` for both
        // the vault contract and the oracle registry. Byte-identical to the
        // EVM path.
        if target[19] == 0x0F {
            struct VaultAdapter<'a, H: ZvmHost + ?Sized> {
                host: &'a H,
                ts: u64,
            }
            impl<H: ZvmHost + ?Sized> zbx_crypto::vault_state::VaultStateReader
                for VaultAdapter<'_, H>
            {
                fn read_slot(&self, addr: &[u8; 20], slot: &[u8; 32]) -> [u8; 32] {
                    self.host.storage_load(addr, slot)
                }
                fn current_timestamp(&self) -> u64 { self.ts }
            }
            let adapter = VaultAdapter { host: &*self.host, ts: self.ctx.block_timestamp };
            match crate::precompiles::zusd_vault_with(calldata, avail, &adapter) {
                Ok((output, gas_used)) => {
                    self.gas = self.gas.saturating_sub(gas_used);
                    let copy_len = ret_len.min(output.len());
                    for i in 0..copy_len {
                        self.memory.store8(ret_off + i, output[i])?;
                    }
                    self.return_data = output;
                    self.stack.push_u64(1)?;
                }
                Err(_) => {
                    self.return_data.clear();
                    self.stack.push_u64(0)?;
                }
            }
            return Ok(());
        }

        match crate::precompiles::call_precompile(target, calldata, avail) {
            Ok((output, gas_used)) => {
                self.gas = self.gas.saturating_sub(gas_used);
                let copy_len = ret_len.min(output.len());
                for i in 0..copy_len {
                    self.memory.store8(ret_off + i, output[i])?;
                }
                self.return_data = output;
                self.stack.push_u64(1)?;
            }
            Err(_) => {
                // Precompile rejected (fail-closed) — consume nothing,
                // return empty data and push 0 so the caller can branch.
                self.return_data.clear();
                self.stack.push_u64(0)?;
            }
        }
        Ok(())
    }

    fn finish(&self, status: ExecutionStatus) -> ZvmResult {
        let gas_used = self.ctx.gas_limit.saturating_sub(self.gas);
        ZvmResult {
            status,
            return_data: self.return_data.clone(),
            gas_remaining: self.gas,
            gas_used,
            logs: self.logs.clone(),
            zvm_logs: self.zvm_logs.clone(),
        }
    }
}

// ─── SEC-2026-05-09 Pass-16: U256 signed-arithmetic helpers ──────────────────
//
// EVM treats stack words as two's-complement signed integers for SDIV / SMOD /
// SLT / SGT / SAR. `primitive_types::U256` only exposes unsigned ops, so we
// reinterpret the high bit as the sign and do magnitude math + sign-fixup.

#[inline]
fn is_negative(x: U256) -> bool {
    let mut be = [0u8; 32];
    x.to_big_endian(&mut be);
    be[0] & 0x80 != 0
}

#[inline]
fn neg(x: U256) -> U256 {
    // Two's complement: !x + 1 (wrap on 0).
    (!x).overflowing_add(U256::one()).0
}

fn sdiv_u256(a: U256, b: U256) -> U256 {
    if b.is_zero() { return U256::zero(); }
    let min_neg = U256::from(1u8) << 255;
    // Special case: MIN_NEG / -1 = MIN_NEG (Yellow Paper §H).
    let neg_one = U256::max_value();
    if a == min_neg && b == neg_one { return min_neg; }
    let neg_a = is_negative(a);
    let neg_b = is_negative(b);
    let abs_a = if neg_a { neg(a) } else { a };
    let abs_b = if neg_b { neg(b) } else { b };
    let q = abs_a / abs_b;
    if neg_a ^ neg_b { neg(q) } else { q }
}

fn smod_u256(a: U256, b: U256) -> U256 {
    if b.is_zero() { return U256::zero(); }
    let neg_a = is_negative(a);
    let neg_b = is_negative(b);
    let abs_a = if neg_a { neg(a) } else { a };
    let abs_b = if neg_b { neg(b) } else { b };
    let r = abs_a % abs_b;
    if r.is_zero() { U256::zero() } else if neg_a { neg(r) } else { r }
}

fn signed_lt(a: U256, b: U256) -> bool {
    let na = is_negative(a);
    let nb = is_negative(b);
    match (na, nb) {
        (true, false) => true,
        (false, true) => false,
        _             => a < b,
    }
}

fn sar_u256(value: U256, shift: U256) -> U256 {
    let neg_v = is_negative(value);
    if shift >= U256::from(256u64) {
        return if neg_v { U256::max_value() } else { U256::zero() };
    }
    let s = shift.low_u64() as usize;
    let logical = value >> s;
    if !neg_v { return logical; }
    // s == 0: nothing to sign-fill (and `<< 256` would overflow).
    if s == 0 { return logical; }
    // Sign-fill the top `s` bits with 1s.
    let mask = (!U256::zero()) << (256 - s);
    logical | mask
}

// ─── Address derivation helpers ───────────────────────────────────────────────

/// CREATE address: keccak256(rlp([creator, nonce]))[12..]
fn create_address(creator: &[u8; 20], nonce: u64) -> [u8; 20] {
    let mut hasher = Keccak256::new();
    // Minimal RLP: [creator (20 bytes), nonce (compact u64)]
    let nonce_bytes = nonce.to_be_bytes();
    let nonce_compact: &[u8] = {
        let leading = nonce_bytes.iter().take_while(|&&b| b == 0).count();
        &nonce_bytes[leading.min(7)..]
    };
    // RLP-encode as a list of two items.
    let creator_rlp = {
        let mut v = Vec::with_capacity(21);
        v.push(0x80 + 20u8);
        v.extend_from_slice(creator);
        v
    };
    let nonce_rlp = if nonce == 0 {
        vec![0x80u8]
    } else if nonce_compact.len() == 1 && nonce_compact[0] < 0x80 {
        vec![nonce_compact[0]]
    } else {
        let mut v = Vec::with_capacity(1 + nonce_compact.len());
        v.push(0x80 + nonce_compact.len() as u8);
        v.extend_from_slice(nonce_compact);
        v
    };
    let payload_len = creator_rlp.len() + nonce_rlp.len();
    let mut list = Vec::with_capacity(1 + payload_len);
    list.push(0xC0 + payload_len as u8);
    list.extend_from_slice(&creator_rlp);
    list.extend_from_slice(&nonce_rlp);
    hasher.update(&list);
    let h = hasher.finalize();
    let mut addr = [0u8; 20];
    addr.copy_from_slice(&h[12..]);
    addr
}

/// CREATE2 address: keccak256(0xff ++ creator ++ salt ++ keccak256(initcode))[12..]
fn create2_address(creator: &[u8; 20], salt: &[u8; 32], initcode: &[u8]) -> [u8; 20] {
    let init_hash = Keccak256::digest(initcode);
    let mut hasher = Keccak256::new();
    hasher.update([0xFFu8]);
    hasher.update(creator);
    hasher.update(salt);
    hasher.update(&init_hash);
    let h = hasher.finalize();
    let mut addr = [0u8; 20];
    addr.copy_from_slice(&h[12..]);
    addr
}
