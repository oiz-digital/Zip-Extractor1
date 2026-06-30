//! ZVM memory — linear byte array, grows in 32-byte chunks.

use crate::error::ZvmError;

/// ZVM memory — same semantics as EVM memory.
pub struct ZvmMemory {
    data: Vec<u8>,
}

impl ZvmMemory {
    pub fn new() -> Self {
        ZvmMemory { data: Vec::new() }
    }

    /// Ensure memory is at least `size` bytes. Grows in 32-byte words.
    pub fn resize(&mut self, size: usize) {
        if size > self.data.len() {
            // Round up to 32-byte boundary
            let new_size = (size + 31) / 32 * 32;
            self.data.resize(new_size, 0);
        }
    }

    /// Read 32 bytes from memory at offset.
    pub fn load(&mut self, offset: usize) -> Result<[u8; 32], ZvmError> {
        self.resize(offset + 32);
        let mut word = [0u8; 32];
        word.copy_from_slice(&self.data[offset..offset + 32]);
        Ok(word)
    }

    /// Write 32 bytes to memory at offset.
    pub fn store(&mut self, offset: usize, word: [u8; 32]) -> Result<(), ZvmError> {
        self.resize(offset + 32);
        self.data[offset..offset + 32].copy_from_slice(&word);
        Ok(())
    }

    /// Write 1 byte to memory at offset.
    pub fn store8(&mut self, offset: usize, byte: u8) -> Result<(), ZvmError> {
        self.resize(offset + 1);
        self.data[offset] = byte;
        Ok(())
    }

    /// Read a slice of bytes from memory.
    pub fn read_slice(&mut self, offset: usize, len: usize) -> &[u8] {
        self.resize(offset + len);
        &self.data[offset..offset + len]
    }

    /// Read bytes as a UTF-8 string (for ZVM opcodes like PAYID, ZVMLOG).
    pub fn read_string(&mut self, offset: usize, len: usize) -> Result<String, ZvmError> {
        let bytes = self.read_slice(offset, len).to_vec();
        String::from_utf8(bytes).map_err(|_| ZvmError::InvalidUtf8)
    }

    /// Current memory size in bytes.
    pub fn size(&self) -> usize { self.data.len() }

    /// Memory size in 32-byte words (for gas calculation).
    pub fn words(&self) -> usize { (self.data.len() + 31) / 32 }

    /// Write an arbitrary byte slice at `offset` (resizing as needed).
    /// Used by CALLDATACOPY / CODECOPY / RETURNDATACOPY / EXTCODECOPY.
    pub fn write_slice(&mut self, offset: usize, src: &[u8]) -> Result<(), ZvmError> {
        if src.is_empty() { return Ok(()); }
        self.resize(offset + src.len());
        self.data[offset..offset + src.len()].copy_from_slice(src);
        Ok(())
    }

    /// EIP-5656 MCOPY: copy `len` bytes from `src` to `dst` within memory,
    /// supporting overlapping ranges (uses `copy_within`).
    pub fn copy(&mut self, dst: usize, src: usize, len: usize) -> Result<(), ZvmError> {
        if len == 0 { return Ok(()); }
        let end = dst.max(src).saturating_add(len);
        self.resize(end);
        self.data.copy_within(src..src + len, dst);
        Ok(())
    }
}

impl Default for ZvmMemory {
    fn default() -> Self { Self::new() }
}