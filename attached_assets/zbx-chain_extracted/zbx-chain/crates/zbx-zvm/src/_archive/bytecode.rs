//! ZvmBytecode — typed wrapper for ZVM native bytecode.
//!
//! ZVM bytecode starts with ZVM_MAGIC [0xEF, 0x5A, 0x42] and contains
//! ZVM-specific opcodes (0xC0-0xCA) in addition to standard EVM opcodes.
//!
//! # Bytecode Format
//! ```
//! [magic: 3 bytes][version: 1 byte][sections...]
//! magic:   EF 5A 42   ("EFZ B" = Effective ZBX)
//! version: 0x01       (ZVM version 1)
//! ```

use crate::ZVM_MAGIC;

/// Typed ZVM bytecode container — guaranteed to start with ZVM_MAGIC.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZvmBytecode {
    raw: Vec<u8>,
}

impl ZvmBytecode {
    /// Version byte for ZVM v1.
    pub const ZVM_VERSION_1: u8 = 0x01;

    /// Parse and validate ZVM bytecode.
    /// Returns Err if the magic header is missing or version is unsupported.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, ZvmBytecodeError> {
        if bytes.len() < 4 {
            return Err(ZvmBytecodeError::TooShort(bytes.len()));
        }
        if &bytes[..3] != &ZVM_MAGIC {
            return Err(ZvmBytecodeError::InvalidMagic(
                [bytes[0], bytes[1], bytes[2]]
            ));
        }
        if bytes[3] != Self::ZVM_VERSION_1 {
            return Err(ZvmBytecodeError::UnsupportedVersion(bytes[3]));
        }
        Ok(Self { raw: bytes })
    }

    /// Create ZvmBytecode from raw bytes, wrapping with magic if absent.
    pub fn from_raw_or_wrap(bytes: Vec<u8>) -> Self {
        if bytes.len() >= 3 && &bytes[..3] == &ZVM_MAGIC {
            // Already has magic — assume valid
            Self { raw: bytes }
        } else {
            // Prepend magic + version
            let mut wrapped = vec![ZVM_MAGIC[0], ZVM_MAGIC[1], ZVM_MAGIC[2], Self::ZVM_VERSION_1];
            wrapped.extend_from_slice(&bytes);
            Self { raw: wrapped }
        }
    }

    /// Returns the raw bytecode bytes (including magic header).
    pub fn as_bytes(&self) -> &[u8] { &self.raw }

    /// Returns the bytecode body (without magic + version bytes).
    pub fn body(&self) -> &[u8] { &self.raw[4..] }

    /// Returns the ZVM version byte.
    pub fn version(&self) -> u8 { self.raw[3] }

    /// Returns true if this is ZVM native bytecode (has magic header).
    pub fn is_zvm(&self) -> bool {
        self.raw.len() >= 3 && &self.raw[..3] == &ZVM_MAGIC
    }

    /// Total size in bytes.
    pub fn len(&self) -> usize { self.raw.len() }

    /// Returns true if bytecode is empty.
    pub fn is_empty(&self) -> bool { self.raw.is_empty() }
}

#[derive(Debug, thiserror::Error)]
pub enum ZvmBytecodeError {
    #[error("bytecode too short: {0} bytes (need ≥4)")]
    TooShort(usize),
    #[error("invalid ZVM magic: {0:02x?} (expected EF 5A 42)")]
    InvalidMagic([u8; 3]),
    #[error("unsupported ZVM version: {0:#x}")]
    UnsupportedVersion(u8),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_zvm_bytes() -> Vec<u8> {
        vec![0xEF, 0x5A, 0x42, 0x01, 0x60, 0x01, 0x60, 0x02, 0x01] // PUSH1 1 PUSH1 2 ADD
    }

    #[test]
    fn valid_bytecode_parses() {
        let bc = ZvmBytecode::from_bytes(valid_zvm_bytes()).unwrap();
        assert!(bc.is_zvm());
        assert_eq!(bc.version(), 0x01);
        assert_eq!(bc.body().len(), 5); // without 4-byte header
    }

    #[test]
    fn wrong_magic_rejected() {
        let bad = vec![0xDE, 0xAD, 0xBE, 0x01, 0x00];
        let err = ZvmBytecode::from_bytes(bad).unwrap_err();
        assert!(matches!(err, ZvmBytecodeError::InvalidMagic(_)));
    }

    #[test]
    fn too_short_rejected() {
        let err = ZvmBytecode::from_bytes(vec![0xEF, 0x5A]).unwrap_err();
        assert!(matches!(err, ZvmBytecodeError::TooShort(2)));
    }

    #[test]
    fn from_raw_wraps_magic() {
        let raw_evm = vec![0x60, 0x01, 0x60, 0x02, 0x01];
        let bc = ZvmBytecode::from_raw_or_wrap(raw_evm);
        assert!(bc.is_zvm());
        assert_eq!(bc.version(), ZvmBytecode::ZVM_VERSION_1);
    }

    #[test]
    fn unsupported_version_rejected() {
        let bad_ver = vec![0xEF, 0x5A, 0x42, 0xFF, 0x00];
        let err = ZvmBytecode::from_bytes(bad_ver).unwrap_err();
        assert!(matches!(err, ZvmBytecodeError::UnsupportedVersion(0xFF)));
    }
}