//! Borsh — Binary Object Representation Serializer for Hashing.
//! Used for cross-chain messages to Solana/Near ecosystem.

use crate::CodecError;

/// Borsh-encode a u8.
pub fn encode_u8(v: u8) -> [u8; 1] { [v] }
/// Borsh-encode a u32.
pub fn encode_u32(v: u32) -> [u8; 4] { v.to_le_bytes() }
/// Borsh-encode a u64.
pub fn encode_u64(v: u64) -> [u8; 8] { v.to_le_bytes() }
/// Borsh-encode a u128.
pub fn encode_u128(v: u128) -> [u8; 16] { v.to_le_bytes() }

/// Borsh-encode a byte string (prefixed with 4-byte length).
pub fn encode_bytes(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + data.len());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
    out
}

/// Borsh-encode a vector of items (each encoded by `f`).
pub fn encode_vec<T, F: Fn(&T) -> Vec<u8>>(items: &[T], f: F) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(items.len() as u32).to_le_bytes());
    for item in items { out.extend(f(item)); }
    out
}

/// Borsh-decode a u64 from bytes at offset.
pub fn decode_u64(bytes: &[u8], offset: usize) -> Result<(u64, usize), CodecError> {
    if offset + 8 > bytes.len() {
        return Err(CodecError::BufferTooShort { need: offset + 8, got: bytes.len() });
    }
    let v = u64::from_le_bytes(bytes[offset..offset+8].try_into().unwrap());
    Ok((v, offset + 8))
}

/// Borsh-decode a byte string.
pub fn decode_bytes(bytes: &[u8], offset: usize) -> Result<(Vec<u8>, usize), CodecError> {
    if offset + 4 > bytes.len() {
        return Err(CodecError::BufferTooShort { need: offset + 4, got: bytes.len() });
    }
    let len = u32::from_le_bytes(bytes[offset..offset+4].try_into().unwrap()) as usize;
    let end = offset + 4 + len;
    if end > bytes.len() {
        return Err(CodecError::BufferTooShort { need: end, got: bytes.len() });
    }
    Ok((bytes[offset+4..end].to_vec(), end))
}