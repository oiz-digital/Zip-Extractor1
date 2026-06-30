//! SSZ (Simple Serialize) — Ethereum 2 consensus layer format.
//!
//! SSZ is used for:
//!   - Beacon chain block headers (interop with Eth2 light clients)
//!   - ZK proof inputs (deterministic serialisation)
//!   - Cross-chain bridge proofs to Ethereum L1
//!
//! Key properties:
//!   - Deterministic (same input → always same bytes)
//!   - Merkleizable (each field maps to a 32-byte chunk)
//!   - Fixed-length types: packed without length prefix
//!   - Variable-length types: offset + data in tail

use crate::CodecError;

/// SSZ-encode a u64.
pub fn encode_u64(v: u64) -> [u8; 8] { v.to_le_bytes() }

/// SSZ-encode a u128.
pub fn encode_u128(v: u128) -> [u8; 16] { v.to_le_bytes() }

/// SSZ-encode a fixed-size array.
pub fn encode_fixed<const N: usize>(arr: [u8; N]) -> [u8; N] { arr }

/// SSZ-encode a variable-length byte list (with 4-byte length prefix in container).
pub fn encode_byte_list(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + data.len());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
    out
}

/// SSZ-decode a u64.
pub fn decode_u64(bytes: &[u8]) -> Result<u64, CodecError> {
    bytes[..8].try_into()
        .map(u64::from_le_bytes)
        .map_err(|_| CodecError::BufferTooShort { need: 8, got: bytes.len() })
}

/// Compute SSZ Merkle tree root of a list of 32-byte chunks.
pub fn merkle_root(chunks: &[[u8; 32]]) -> [u8; 32] {
    if chunks.is_empty() { return [0u8; 32]; }
    let mut layer: Vec<[u8; 32]> = chunks.to_vec();
    // Pad to power of 2.
    while layer.len() & (layer.len() - 1) != 0 { layer.push([0u8; 32]); }
    while layer.len() > 1 {
        let mut next = vec![];
        for pair in layer.chunks(2) {
            let mut input = [0u8; 64];
            input[..32].copy_from_slice(&pair[0]);
            input[32..].copy_from_slice(&pair[1]);
            next.push(sha256(&input));
        }
        layer = next;
    }
    layer[0]
}

fn sha256(data: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(data);
    let out = h.finalize();
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&out);
    arr
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn u64_roundtrip() {
        let v = 0xDEADBEEFu64;
        let enc = encode_u64(v);
        let dec = decode_u64(&enc).unwrap();
        assert_eq!(v, dec);
    }
    #[test] fn single_chunk_root() {
        let chunk = [1u8; 32];
        let root = merkle_root(&[chunk]);
        assert_eq!(root, chunk, "single chunk root = chunk itself");
    }
}