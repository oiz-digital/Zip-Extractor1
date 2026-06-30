//! 4-bit nibble decomposition of byte-string keys.

use std::fmt;

/// A sequence of 4-bit nibbles representing a trie path.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Nibbles {
    inner: Vec<u8>,
    /// Byte offset into inner (0 or 1) for half-byte alignment.
    offset: usize,
}

impl Nibbles {
    /// Create nibbles from a raw byte slice (2 nibbles per byte).
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let mut inner = Vec::with_capacity(bytes.len() * 2);
        for &b in bytes {
            inner.push(b >> 4);
            inner.push(b & 0x0f);
        }
        Self { inner, offset: 0 }
    }

    /// Number of nibbles.
    pub fn len(&self) -> usize {
        self.inner.len().saturating_sub(self.offset)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get nibble at position `i`.
    pub fn at(&self, i: usize) -> u8 {
        self.inner[self.offset + i]
    }

    /// Return nibbles starting at index `i`.
    pub fn slice(&self, i: usize) -> Self {
        Self {
            inner: self.inner.clone(),
            offset: self.offset + i,
        }
    }

    /// Length of the common prefix shared with `other`.
    pub fn common_prefix_len(&self, other: &Nibbles) -> usize {
        let max = self.len().min(other.len());
        (0..max).take_while(|&i| self.at(i) == other.at(i)).count()
    }

    // ---- W1.5 (S33-state-root): subslicing + composition helpers ----

    /// Empty nibbles. Used for branch-collapse → leaf-with-empty-path
    /// and as the initial value for accumulators.
    pub fn empty() -> Self {
        Self { inner: Vec::new(), offset: 0 }
    }

    /// A single-nibble path. Used by branch collapse during `delete()`
    /// to prepend the surviving child's branch slot index onto a merged
    /// leaf/extension partial.
    pub fn single(nibble: u8) -> Self {
        Self { inner: vec![nibble & 0x0f], offset: 0 }
    }

    /// Sub-slice: nibbles `[start..start+len]` of `self`.
    /// Replaces the broken `key.slice(depth).slice(0).slice(0)` pattern
    /// at the previous `trie.rs:113`. Materialises into a fresh `Vec`
    /// so the result is independent of the source.
    pub fn sub(&self, start: usize, len: usize) -> Self {
        let from = self.offset + start;
        let to = (from + len).min(self.inner.len());
        Self {
            inner: self.inner[from..to].to_vec(),
            offset: 0,
        }
    }

    /// Concatenation: `self` followed by `other`. Used by extension /
    /// branch collapse logic to merge partial paths during `delete()`.
    pub fn concat(&self, other: &Self) -> Self {
        let mut inner = Vec::with_capacity(self.len() + other.len());
        for i in 0..self.len() {
            inner.push(self.at(i));
        }
        for i in 0..other.len() {
            inner.push(other.at(i));
        }
        Self { inner, offset: 0 }
    }

    /// Encode nibbles using the HP (hex-prefix) encoding.
    /// `leaf` flag indicates whether this is a leaf node path.
    pub fn encode_compact(&self, leaf: bool) -> Vec<u8> {
        let odd = self.len() % 2 == 1;
        let flag = if leaf { 0x20 } else { 0x00 } | if odd { 0x10 } else { 0x00 };
        let mut out = Vec::with_capacity(1 + (self.len() + 1) / 2);
        if odd {
            out.push(flag | self.at(0));
            for i in (1..self.len()).step_by(2) {
                out.push((self.at(i) << 4) | self.at(i + 1));
            }
        } else {
            out.push(flag);
            for i in (0..self.len()).step_by(2) {
                out.push((self.at(i) << 4) | self.at(i + 1));
            }
        }
        out
    }

    /// Decode HP-encoded compact nibbles.
    pub fn decode_compact(encoded: &[u8]) -> (Self, bool) {
        assert!(!encoded.is_empty(), "empty compact encoding");
        let flag = encoded[0];
        let leaf = flag & 0x20 != 0;
        let odd  = flag & 0x10 != 0;
        let mut nibs = Vec::new();
        if odd {
            nibs.push(flag & 0x0f);
        }
        for &b in &encoded[1..] {
            nibs.push(b >> 4);
            nibs.push(b & 0x0f);
        }
        (Self { inner: nibs, offset: 0 }, leaf)
    }
}

impl fmt::Debug for Nibbles {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Nibbles(")?;
        for i in 0..self.len() {
            write!(f, "{:x}", self.at(i))?;
        }
        write!(f, ")")
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_bytes_length() {
        let n = Nibbles::from_bytes(&[0xab, 0xcd]);
        assert_eq!(n.len(), 4);
        assert_eq!(n.at(0), 0xa);
        assert_eq!(n.at(1), 0xb);
        assert_eq!(n.at(2), 0xc);
        assert_eq!(n.at(3), 0xd);
    }

    #[test]
    fn empty_nibbles() {
        let n = Nibbles::empty();
        assert!(n.is_empty());
        assert_eq!(n.len(), 0);
    }

    #[test]
    fn single_nibble() {
        let n = Nibbles::single(0x7);
        assert_eq!(n.len(), 1);
        assert_eq!(n.at(0), 0x7);
    }

    #[test]
    fn slice_advances_offset() {
        let n = Nibbles::from_bytes(&[0xab, 0xcd]);
        let s = n.slice(2);
        assert_eq!(s.len(), 2);
        assert_eq!(s.at(0), 0xc);
    }

    #[test]
    fn sub_extracts_range() {
        let n = Nibbles::from_bytes(&[0xab, 0xcd]);
        let sub = n.sub(1, 2);
        assert_eq!(sub.len(), 2);
        assert_eq!(sub.at(0), 0xb);
        assert_eq!(sub.at(1), 0xc);
    }

    #[test]
    fn common_prefix_len() {
        let a = Nibbles::from_bytes(&[0xab, 0xcd]);
        let b = Nibbles::from_bytes(&[0xab, 0xef]);
        assert_eq!(a.common_prefix_len(&b), 2);
    }

    #[test]
    fn concat_joins() {
        let a = Nibbles::from_bytes(&[0xab]);
        let b = Nibbles::from_bytes(&[0xcd]);
        let c = a.concat(&b);
        assert_eq!(c.len(), 4);
        assert_eq!(c.at(0), 0xa);
        assert_eq!(c.at(3), 0xd);
    }

    #[test]
    fn hp_encode_decode_leaf_even() {
        let n = Nibbles::from_bytes(&[0xab]);
        let enc = n.encode_compact(true);
        let (dec, leaf) = Nibbles::decode_compact(&enc);
        assert!(leaf);
        assert_eq!(dec.len(), n.len());
        for i in 0..n.len() { assert_eq!(dec.at(i), n.at(i)); }
    }

    #[test]
    fn hp_encode_decode_extension_odd() {
        let n = Nibbles::single(0x5);
        let enc = n.encode_compact(false);
        let (dec, leaf) = Nibbles::decode_compact(&enc);
        assert!(!leaf);
        assert_eq!(dec.len(), 1);
        assert_eq!(dec.at(0), 0x5);
    }
}
