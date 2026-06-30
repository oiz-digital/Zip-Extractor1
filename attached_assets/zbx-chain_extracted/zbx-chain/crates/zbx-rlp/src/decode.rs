//! RLP decoding — zero-copy view.

use crate::error::RlpError;

/// Zero-copy RLP data view.
pub struct Rlp<'a> {
    data: &'a [u8],
}

pub trait Decodable: Sized {
    fn decode_from(rlp: &Rlp<'_>) -> Result<Self, RlpError>;
}

impl<'a> Rlp<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data }
    }

    /// Raw RLP bytes of this item (full serialization, including length
    /// header). Useful for recursively decoding inline list items embedded
    /// inside another list — e.g. inline trie children in a Branch node,
    /// where `val_at::<Vec<u8>>` would fail with `ExpectedString`.
    /// Added 2026-05-09 (Pass-8) to fix S38-TRIE-REGRESSION.
    pub fn as_raw(&self) -> &'a [u8] {
        self.data
    }

    /// Determine whether this RLP item is a string/byte-array.
    pub fn is_data(&self) -> bool {
        self.data.first().map(|&b| b < 0xc0).unwrap_or(false)
    }

    /// Determine whether this RLP item is a list.
    pub fn is_list(&self) -> bool {
        self.data.first().map(|&b| b >= 0xc0).unwrap_or(false)
    }

    /// Decode as raw bytes.
    pub fn as_bytes(&self) -> Result<&'a [u8], RlpError> {
        let (payload, _) = self.split_payload()?;
        Ok(payload)
    }

    /// Decode as Vec<u8>.
    pub fn val_at<T: Decodable>(&self, idx: usize) -> Result<T, RlpError> {
        let item = self.at(idx)?;
        T::decode_from(&item)
    }

    /// Number of items in this list.
    pub fn item_count(&self) -> Result<usize, RlpError> {
        if self.data.is_empty() {
            return Ok(0);
        }
        let b = self.data[0];
        if b < 0xc0 {
            return Ok(0); // Not a list — treat as single item.
        }
        let (list_bytes, _) = self.list_payload()?;
        let mut count = 0;
        let mut pos = 0;
        while pos < list_bytes.len() {
            let item_len = Self::item_length(list_bytes, pos)?;
            pos += item_len;
            count += 1;
        }
        Ok(count)
    }

    /// Get the `n`th item in a list.
    pub fn at(&self, n: usize) -> Result<Rlp<'a>, RlpError> {
        let (list_bytes, _) = self.list_payload()?;
        let mut pos = 0;
        let mut idx = 0;
        while pos < list_bytes.len() {
            let item_len = Self::item_length(list_bytes, pos)?;
            if idx == n {
                return Ok(Rlp::new(&list_bytes[pos..pos + item_len]));
            }
            pos += item_len;
            idx += 1;
        }
        Err(RlpError::ItemCountMismatch)
    }

    fn item_length(data: &[u8], pos: usize) -> Result<usize, RlpError> {
        if pos >= data.len() {
            return Err(RlpError::UnexpectedEnd(pos));
        }
        let b = data[pos];
        // SEC-2026-05-09 Pass-8 (S38 follow-up): every branch must
        // guarantee `pos + item_len <= data.len()` so callers like
        // `at()` and `as_bytes()` can slice without panicking on
        // malformed P2P / RPC input. The previous short-string and
        // short-list cases returned a length without validating it.
        let item_len: usize = if b < 0x80 {
            1
        } else if b < 0xb8 {
            1 + (b - 0x80) as usize
        } else if b < 0xc0 {
            let len_bytes = (b - 0xb7) as usize;
            if pos + 1 + len_bytes > data.len() {
                return Err(RlpError::UnexpectedEnd(pos + 1 + len_bytes));
            }
            let len = Self::decode_length(&data[pos+1..pos+1+len_bytes])?;
            1usize.checked_add(len_bytes)
                .and_then(|x| x.checked_add(len))
                .ok_or(RlpError::Overflow)?
        } else if b < 0xf8 {
            1 + (b - 0xc0) as usize
        } else {
            let len_bytes = (b - 0xf7) as usize;
            if pos + 1 + len_bytes > data.len() {
                return Err(RlpError::UnexpectedEnd(pos + 1 + len_bytes));
            }
            let len = Self::decode_length(&data[pos+1..pos+1+len_bytes])?;
            1usize.checked_add(len_bytes)
                .and_then(|x| x.checked_add(len))
                .ok_or(RlpError::Overflow)?
        };
        let end = pos.checked_add(item_len).ok_or(RlpError::Overflow)?;
        if end > data.len() {
            return Err(RlpError::UnexpectedEnd(end));
        }
        Ok(item_len)
    }

    fn decode_length(bytes: &[u8]) -> Result<usize, RlpError> {
        // Canonical RLP forbids leading zero bytes in length encoding —
        // accepting them allows two distinct encodings of the same value
        // (transaction-malleability source). See AUDIT_2026-04-30.md M-03.
        if bytes.is_empty() { return Err(RlpError::UnexpectedEnd(0)); }
        if bytes[0] == 0 { return Err(RlpError::Overflow); }
        let mut val = 0usize;
        for &b in bytes {
            val = val.checked_shl(8).ok_or(RlpError::Overflow)?
                | b as usize;
        }
        Ok(val)
    }

    fn split_payload(&self) -> Result<(&'a [u8], &'a [u8]), RlpError> {
        if self.data.is_empty() {
            return Err(RlpError::UnexpectedEnd(0));
        }
        let b = self.data[0];
        // Each branch validates that the declared payload fits in the buffer
        // before slicing — prevents panic on truncated input.
        // See AUDIT_2026-04-30.md C-14.
        if b < 0x80 {
            Ok((&self.data[0..1], &self.data[1..]))
        } else if b < 0xb8 {
            let len = (b - 0x80) as usize;
            if 1 + len > self.data.len() { return Err(RlpError::UnexpectedEnd(1 + len)); }
            Ok((&self.data[1..1+len], &self.data[1+len..]))
        } else if b < 0xc0 {
            let len_bytes = (b - 0xb7) as usize;
            if 1 + len_bytes > self.data.len() {
                return Err(RlpError::UnexpectedEnd(1 + len_bytes));
            }
            let len = Self::decode_length(&self.data[1..1+len_bytes])?;
            let end = 1usize.checked_add(len_bytes)
                .and_then(|x| x.checked_add(len))
                .ok_or(RlpError::Overflow)?;
            if end > self.data.len() { return Err(RlpError::UnexpectedEnd(end)); }
            Ok((&self.data[1+len_bytes..end], &self.data[end..]))
        } else {
            Err(RlpError::ExpectedString)
        }
    }

    fn list_payload(&self) -> Result<(&'a [u8], &'a [u8]), RlpError> {
        if self.data.is_empty() {
            return Ok((&[], &[]));
        }
        let b = self.data[0];
        if b < 0xc0 {
            return Err(RlpError::ExpectedList);
        }
        if b < 0xf8 {
            let len = (b - 0xc0) as usize;
            if 1 + len > self.data.len() { return Err(RlpError::UnexpectedEnd(1 + len)); }
            Ok((&self.data[1..1+len], &self.data[1+len..]))
        } else {
            let len_bytes = (b - 0xf7) as usize;
            if 1 + len_bytes > self.data.len() {
                return Err(RlpError::UnexpectedEnd(1 + len_bytes));
            }
            let len = Self::decode_length(&self.data[1..1+len_bytes])?;
            let end = 1usize.checked_add(len_bytes)
                .and_then(|x| x.checked_add(len))
                .ok_or(RlpError::Overflow)?;
            if end > self.data.len() { return Err(RlpError::UnexpectedEnd(end)); }
            Ok((&self.data[1+len_bytes..end], &self.data[end..]))
        }
    }
}

impl Decodable for Vec<u8> {
    fn decode_from(rlp: &Rlp<'_>) -> Result<Self, RlpError> {
        Ok(rlp.as_bytes()?.to_vec())
    }
}

impl Decodable for u64 {
    fn decode_from(rlp: &Rlp<'_>) -> Result<Self, RlpError> {
        let bytes = rlp.as_bytes()?;
        if bytes.len() > 8 {
            return Err(RlpError::Overflow);
        }
        let mut val = 0u64;
        for &b in bytes {
            val = (val << 8) | b as u64;
        }
        Ok(val)
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_single_byte_is_data() {
        let rlp = Rlp::new(&[0x42]);
        assert!(rlp.is_data());
        assert!(!rlp.is_list());
    }

    #[test]
    fn decode_empty_string() {
        let rlp = Rlp::new(&[0x80]);
        assert!(rlp.is_data());
        assert_eq!(rlp.as_bytes().unwrap(), &[] as &[u8]);
    }

    #[test]
    fn decode_short_string() {
        // "hello" encoded: 0x85 followed by "hello"
        let encoded = [0x85, b'h', b'e', b'l', b'l', b'o'];
        let rlp = Rlp::new(&encoded);
        assert!(rlp.is_data());
        assert_eq!(rlp.as_bytes().unwrap(), b"hello");
    }

    #[test]
    fn decode_empty_list() {
        let rlp = Rlp::new(&[0xc0]);
        assert!(rlp.is_list());
        assert_eq!(rlp.item_count().unwrap(), 0);
    }

    #[test]
    fn decode_list_two_bytes() {
        // [0x01, 0x02] → 0xc2 0x01 0x02
        let encoded = [0xc2, 0x01, 0x02];
        let rlp = Rlp::new(&encoded);
        assert!(rlp.is_list());
        assert_eq!(rlp.item_count().unwrap(), 2);
        assert_eq!(rlp.at(0).unwrap().as_bytes().unwrap(), &[0x01]);
        assert_eq!(rlp.at(1).unwrap().as_bytes().unwrap(), &[0x02]);
    }

    #[test]
    fn as_raw_returns_full_bytes() {
        let encoded = [0x85, b'h', b'e', b'l', b'l', b'o'];
        let rlp = Rlp::new(&encoded);
        assert_eq!(rlp.as_raw(), &encoded);
    }
}
