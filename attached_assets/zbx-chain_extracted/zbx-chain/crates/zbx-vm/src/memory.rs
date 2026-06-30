//! EVM linear memory with expansion gas tracking.

use thiserror::Error;

/// Maximum EVM memory size (prevent DoS). 128 MB.
const MAX_MEMORY_SIZE: usize = 128 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum MemoryError {
    #[error("memory expansion exceeds limit ({MAX_MEMORY_SIZE} bytes)")]
    TooLarge,
    #[error("out-of-bounds memory access")]
    OutOfBounds,
}

/// EVM linear memory (byte-addressed, zero-padded on expansion).
pub struct Memory {
    data:    Vec<u8>,
    /// Words already charged for (tracks expansion).
    charged_words: usize,
}

impl Memory {
    pub fn new() -> Self {
        Self { data: Vec::new(), charged_words: 0 }
    }

    /// Compute the gas cost for expanding memory to hold `offset + size` bytes.
    /// Returns the incremental gas and updates internal charged word count.
    pub fn expansion_gas(&mut self, offset: usize, size: usize) -> Result<u64, MemoryError> {
        if size == 0 { return Ok(0); }
        let new_size = offset.checked_add(size).ok_or(MemoryError::TooLarge)?;
        if new_size > MAX_MEMORY_SIZE { return Err(MemoryError::TooLarge); }
        let new_words = (new_size + 31) / 32;
        if new_words <= self.charged_words { return Ok(0); }
        let cost = memory_gas_cost(new_words) - memory_gas_cost(self.charged_words);
        self.charged_words = new_words;
        Ok(cost)
    }

    /// Ensure memory is large enough, zero-extending as needed.
    pub fn ensure(&mut self, offset: usize, size: usize) -> Result<(), MemoryError> {
        if size == 0 { return Ok(()); }
        let new_len = offset.checked_add(size).ok_or(MemoryError::TooLarge)?;
        if new_len > MAX_MEMORY_SIZE { return Err(MemoryError::TooLarge); }
        if new_len > self.data.len() {
            self.data.resize(new_len, 0);
        }
        Ok(())
    }

    pub fn get_slice(&self, offset: usize, size: usize) -> &[u8] {
        if size == 0 || offset >= self.data.len() { return &[]; }
        let end = (offset + size).min(self.data.len());
        &self.data[offset..end]
    }

    pub fn set_slice(&mut self, offset: usize, data: &[u8]) {
        if data.is_empty() { return; }
        let end = offset + data.len();
        if end > self.data.len() { self.data.resize(end, 0); }
        self.data[offset..end].copy_from_slice(data);
    }

    pub fn get_u256(&self, offset: usize) -> zbx_types::U256 {
        let mut buf = [0u8; 32];
        let src = self.get_slice(offset, 32);
        buf[..src.len()].copy_from_slice(src);
        zbx_types::U256::from_big_endian(&buf)
    }

    pub fn set_u256(&mut self, offset: usize, val: zbx_types::U256) {
        let mut buf = [0u8; 32];
        val.to_big_endian(&mut buf);
        self.set_slice(offset, &buf);
    }

    pub fn size(&self) -> usize { self.data.len() }
    pub fn data(&self) -> &[u8] { &self.data }
}

/// EVM memory cost formula: 3 * words + floor(words^2 / 512).
fn memory_gas_cost(words: usize) -> u64 {
    let w = words as u64;
    3 * w + w * w / 512
}

impl Default for Memory {
    fn default() -> Self { Self::new() }
}