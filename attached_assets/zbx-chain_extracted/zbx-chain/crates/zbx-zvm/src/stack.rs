//! ZVM execution stack — 256-bit words, max depth 1024.
//!
//! SEC-2026-05-09 Pass-16 (ZVM-U256): real `primitive_types::U256` push/pop
//! helpers. Pre-Pass-16 every arithmetic opcode (ADD/MUL/SUB) downcast the
//! 256-bit stack word to `u128` via `[16..]`, silently truncating the upper
//! 128 bits — a consensus-break vs. Ethereum mainnet for any value ≥ 2^128
//! (e.g. UniswapV3 sqrtPriceX96, MakerDAO RAY math, total-supply over-2^128).

use crate::error::ZvmError;
use primitive_types::U256;

/// Maximum stack depth (same as EVM).
pub const MAX_STACK_DEPTH: usize = 1024;

/// ZVM stack of 256-bit words.
pub struct ZvmStack {
    inner: Vec<[u8; 32]>,
}

impl ZvmStack {
    pub fn new() -> Self {
        ZvmStack { inner: Vec::with_capacity(64) }
    }

    /// Push a 256-bit word onto the stack.
    pub fn push(&mut self, value: [u8; 32]) -> Result<(), ZvmError> {
        if self.inner.len() >= MAX_STACK_DEPTH {
            return Err(ZvmError::StackOverflow);
        }
        self.inner.push(value);
        Ok(())
    }

    /// Push a u64 value (zero-padded to 32 bytes).
    pub fn push_u64(&mut self, value: u64) -> Result<(), ZvmError> {
        let mut word = [0u8; 32];
        word[24..].copy_from_slice(&value.to_be_bytes());
        self.push(word)
    }

    /// Push a u128 value.
    pub fn push_u128(&mut self, value: u128) -> Result<(), ZvmError> {
        let mut word = [0u8; 32];
        word[16..].copy_from_slice(&value.to_be_bytes());
        self.push(word)
    }

    /// Push a 20-byte address (zero-padded to 32 bytes).
    pub fn push_address(&mut self, addr: [u8; 20]) -> Result<(), ZvmError> {
        let mut word = [0u8; 32];
        word[12..].copy_from_slice(&addr);
        self.push(word)
    }

    /// Pop the top word from the stack.
    pub fn pop(&mut self) -> Result<[u8; 32], ZvmError> {
        self.inner.pop().ok_or(ZvmError::StackUnderflow)
    }

    /// Pop as u64 (uses low 8 bytes).
    pub fn pop_u64(&mut self) -> Result<u64, ZvmError> {
        let word = self.pop()?;
        Ok(u64::from_be_bytes(word[24..].try_into().unwrap()))
    }

    /// Pop as u128 (uses low 16 bytes). SEC-2026-05-09 Pass-16: kept only
    /// for ZVM-native opcodes (ZBXBURN, BLOBFEE, ZUSDBAL etc.) that
    /// genuinely fit in 128 bits. EVM arithmetic ops MUST use `pop_u256`.
    pub fn pop_u128(&mut self) -> Result<u128, ZvmError> {
        let word = self.pop()?;
        Ok(u128::from_be_bytes(word[16..].try_into().unwrap()))
    }

    /// Push a full 256-bit value.
    pub fn push_u256(&mut self, value: U256) -> Result<(), ZvmError> {
        let mut word = [0u8; 32];
        value.to_big_endian(&mut word);
        self.push(word)
    }

    /// Pop a full 256-bit value.
    pub fn pop_u256(&mut self) -> Result<U256, ZvmError> {
        let word = self.pop()?;
        Ok(U256::from_big_endian(&word))
    }

    /// Pop as address (uses bytes 12..32).
    pub fn pop_address(&mut self) -> Result<[u8; 20], ZvmError> {
        let word = self.pop()?;
        let mut addr = [0u8; 20];
        addr.copy_from_slice(&word[12..]);
        Ok(addr)
    }

    /// Peek at the top of the stack without popping.
    pub fn peek(&self) -> Result<&[u8; 32], ZvmError> {
        self.inner.last().ok_or(ZvmError::StackUnderflow)
    }

    /// Duplicate item at position n (1-indexed from top).
    pub fn dup(&mut self, n: usize) -> Result<(), ZvmError> {
        let idx = self.inner.len().checked_sub(n).ok_or(ZvmError::StackUnderflow)?;
        let word = self.inner[idx];
        self.push(word)
    }

    /// Swap top with item at position n (1-indexed).
    pub fn swap(&mut self, n: usize) -> Result<(), ZvmError> {
        let len = self.inner.len();
        let idx = len.checked_sub(n + 1).ok_or(ZvmError::StackUnderflow)?;
        self.inner.swap(len - 1, idx);
        Ok(())
    }

    pub fn len(&self) -> usize { self.inner.len() }
    pub fn is_empty(&self) -> bool { self.inner.is_empty() }
}

impl Default for ZvmStack {
    fn default() -> Self { Self::new() }
}