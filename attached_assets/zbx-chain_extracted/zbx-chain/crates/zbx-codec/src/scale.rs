//! SCALE — Simple Concatenated Aggregate Little-Endian codec.
//! Used for cross-chain messages to Polkadot/Substrate ecosystem.

use crate::CodecError;

/// SCALE compact integer encoding (variable-length).
pub fn encode_compact(v: u64) -> Vec<u8> {
    if v < 1 << 6 {
        vec![(v << 2) as u8]
    } else if v < 1 << 14 {
        let x = (v << 2) | 0b01;
        vec![(x & 0xFF) as u8, ((x >> 8) & 0xFF) as u8]
    } else if v < 1 << 30 {
        let x = (v << 2) | 0b10;
        vec![
            (x & 0xFF) as u8, ((x >> 8) & 0xFF) as u8,
            ((x >> 16) & 0xFF) as u8, ((x >> 24) & 0xFF) as u8
        ]
    } else {
        // Big integer mode: length prefix + little-endian bytes
        let bytes = v.to_le_bytes();
        let sig = bytes.iter().rposition(|&b| b != 0).map(|i| i+1).unwrap_or(1);
        let mut out = vec![((sig - 4) << 2 | 0b11) as u8];
        out.extend_from_slice(&bytes[..sig]);
        out
    }
}

/// SCALE-encode a byte vector (compact length prefix + data).
pub fn encode_bytes(data: &[u8]) -> Vec<u8> {
    let mut out = encode_compact(data.len() as u64);
    out.extend_from_slice(data);
    out
}

/// SCALE-encode a bool.
pub fn encode_bool(v: bool) -> [u8; 1] { [v as u8] }

/// SCALE-encode an Option<T>.
pub fn encode_option<T, F: Fn(&T) -> Vec<u8>>(opt: &Option<T>, f: F) -> Vec<u8> {
    match opt {
        None    => vec![0x00],
        Some(v) => { let mut out = vec![0x01]; out.extend(f(v)); out }
    }
}

/// SCALE-decode a compact integer.
pub fn decode_compact(bytes: &[u8], offset: usize) -> Result<(u64, usize), CodecError> {
    if offset >= bytes.len() {
        return Err(CodecError::BufferTooShort { need: offset + 1, got: bytes.len() });
    }
    let first = bytes[offset];
    match first & 0b11 {
        0b00 => Ok(((first >> 2) as u64, offset + 1)),
        0b01 => {
            if offset + 2 > bytes.len() { return Err(CodecError::BufferTooShort { need: offset+2, got: bytes.len() }); }
            let v = (first as u64 | ((bytes[offset+1] as u64) << 8)) >> 2;
            Ok((v, offset + 2))
        }
        0b10 => {
            if offset + 4 > bytes.len() { return Err(CodecError::BufferTooShort { need: offset+4, got: bytes.len() }); }
            let v = (first as u64 | ((bytes[offset+1] as u64) << 8) | ((bytes[offset+2] as u64) << 16) | ((bytes[offset+3] as u64) << 24)) >> 2;
            Ok((v, offset + 4))
        }
        _ => Err(CodecError::SszDecode("big-int compact not supported".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn compact_single_byte() {
        let enc = encode_compact(63);
        assert_eq!(enc, vec![0xFC]);
    }
    #[test] fn compact_two_bytes() {
        let enc = encode_compact(64);
        assert_eq!(enc.len(), 2);
        let (dec, _) = decode_compact(&enc, 0).unwrap();
        assert_eq!(dec, 64);
    }
    #[test] fn roundtrip_bytes() {
        let data = b"hello cross-chain";
        let enc = encode_bytes(data);
        let (len, off) = decode_compact(&enc, 0).unwrap();
        assert_eq!(len as usize, data.len());
        assert_eq!(&enc[off..off + data.len()], data);
    }
}