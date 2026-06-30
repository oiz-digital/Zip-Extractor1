//! Additional stream utilities: typed list helpers, U256, H256 support.

use crate::encode::{Encodable, RlpStream};

/// Encode a homogeneous list of RLP items.
pub fn encode_list<T: Encodable>(items: &[T]) -> Vec<u8> {
    let mut s = RlpStream::new_list(items.len());
    for item in items {
        item.encode_into(&mut s);
    }
    s.out()
}

/// Encode a (big-endian) U256 as a minimal-length RLP bytestring.
pub fn encode_u256(value: [u8; 32]) -> Vec<u8> {
    let leading = value.iter().take_while(|&&b| b == 0).count();
    let trimmed = &value[leading..];
    let mut s = RlpStream::new();
    s.append(trimmed);
    s.out()
}

/// Encode an H256 (fixed 32 bytes, no leading-zero stripping).
pub fn encode_h256(hash: [u8; 32]) -> Vec<u8> {
    let mut s = RlpStream::new();
    s.append(&hash[..]);
    s.out()
}

/// Encode an Ethereum-style address (20 bytes).
pub fn encode_address(addr: [u8; 20]) -> Vec<u8> {
    let mut s = RlpStream::new();
    s.append(&addr[..]);
    s.out()
}