//! EVM linear memory: byte-addressable, grows in 32-byte chunks.

use crate::error::EvmError;
use crate::gas;

const MAX_MEMORY: usize = 32 * 1024 * 1024; // 32 MB

pub struct Memory {
    data: Vec<u8>,
}

impl Memory {
    pub fn new() -> Self { Memory { data: Vec::new() } }

    pub fn size(&self) -> usize { self.data.len() }

    /// Ensure memory is large enough and return the expansion gas cost.
    pub fn ensure(&mut self, offset: usize, size: usize) -> Result<u64, EvmError> {
        if size == 0 { return Ok(0); }
        let end = offset.checked_add(size)
            .ok_or(EvmError::MemoryOutOfBounds { offset, size })?;
        if end > MAX_MEMORY {
            return Err(EvmError::MemoryOutOfBounds { offset, size });
        }
        let old_words = (self.data.len() + 31) / 32;
        let new_words = (end + 31) / 32;
        if new_words > old_words {
            self.data.resize(new_words * 32, 0);
            let cost = gas::memory_expansion_cost(old_words as u64, new_words as u64);
            return Ok(cost);
        }
        Ok(0)
    }

    pub fn read(&self, offset: usize, size: usize) -> Vec<u8> {
        if offset >= self.data.len() {
            return vec![0u8; size];
        }
        let end = (offset + size).min(self.data.len());
        let mut out = self.data[offset..end].to_vec();
        out.resize(size, 0);
        out
    }

    pub fn read32(&self, offset: usize) -> [u8; 32] {
        let b = self.read(offset, 32);
        let mut out = [0u8; 32];
        out.copy_from_slice(&b);
        out
    }

    pub fn write(&mut self, offset: usize, data: &[u8]) {
        if data.is_empty() { return; }
        let end = offset.saturating_add(data.len());
        // L-03 fix: defense-in-depth bounds guard even when ensure() was not called.
        // ensure() is the primary gate; this prevents unbounded allocation from
        // any new opcode implementation that forgets the ensure() call.
        if end > MAX_MEMORY {
            panic!("EVM memory write exceeds MAX_MEMORY ({}): offset={} len={}", MAX_MEMORY, offset, data.len());
        }
        if self.data.len() < end {
            self.data.resize(end, 0);
        }
        self.data[offset..end].copy_from_slice(data);
    }

    pub fn write32(&mut self, offset: usize, val: &[u8; 32]) {
        self.write(offset, val);
    }
}