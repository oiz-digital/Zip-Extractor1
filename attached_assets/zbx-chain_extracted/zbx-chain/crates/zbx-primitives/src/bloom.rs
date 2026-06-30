//! 256-byte (2048-bit) Ethereum-compatible logs bloom filter.

use serde::{Deserialize, Serialize};
use serde_big_array::BigArray;

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct Bloom(#[serde(with = "BigArray")] pub [u8; 256]);

impl Bloom {
    pub const ZERO: Self = Self([0u8; 256]);

    pub fn new() -> Self { Self::ZERO }

    pub fn as_bytes(&self) -> &[u8; 256] { &self.0 }
    pub fn to_bytes(self) -> [u8; 256] { self.0 }
    pub fn is_zero(&self) -> bool { self.0 == [0u8; 256] }

    /// OR another bloom into this one (used when accumulating receipts → block bloom).
    pub fn accrue(&mut self, other: &Bloom) {
        for (a, b) in self.0.iter_mut().zip(other.0.iter()) { *a |= *b; }
    }

    /// Set bits per Ethereum yellow paper: take 3 byte-pairs of keccak256(input)
    /// mod 2048, set those bits.
    pub fn add(&mut self, hashed: &[u8]) {
        if hashed.len() < 6 { return; }
        for i in 0..3 {
            let bit = (((hashed[2 * i] as u16) << 8) | (hashed[2 * i + 1] as u16)) as usize % 2048;
            let byte_idx = 255 - bit / 8;
            let bit_idx = bit % 8;
            self.0[byte_idx] |= 1 << bit_idx;
        }
    }
}

impl Default for Bloom { fn default() -> Self { Self::ZERO } }

impl PartialEq for Bloom { fn eq(&self, other: &Self) -> bool { self.0 == other.0 } }
impl Eq for Bloom {}

impl std::fmt::Debug for Bloom {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Bloom(0x")?;
        for b in &self.0 { write!(f, "{:02x}", b)?; }
        write!(f, ")")
    }
}

impl From<[u8; 256]> for Bloom { fn from(v: [u8; 256]) -> Self { Self(v) } }
impl From<Bloom> for [u8; 256] { fn from(v: Bloom) -> Self { v.0 } }
impl AsRef<[u8]> for Bloom { fn as_ref(&self) -> &[u8] { &self.0 } }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_bloom_is_zero() {
        assert!(Bloom::ZERO.is_zero());
        assert!(Bloom::default().is_zero());
    }

    #[test]
    fn add_sets_bits() {
        let mut b = Bloom::new();
        let data = [1u8, 2, 3, 4, 5, 6, 7, 8];
        b.add(&data);
        assert!(!b.is_zero());
    }

    #[test]
    fn accrue_is_or() {
        let mut a = Bloom::new();
        a.add(&[0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]);
        let mut b = Bloom::new();
        b.add(&[0x11, 0x22, 0x33, 0x44, 0x55, 0x66]);
        let mut c = a;
        c.accrue(&b);
        assert!(!c.is_zero());
    }

    #[test]
    fn roundtrip_bytes() {
        let raw = [0xabu8; 256];
        let bl = Bloom::from(raw);
        let back: [u8; 256] = bl.into();
        assert_eq!(back, raw);
    }

    #[test]
    fn equality() {
        let a = Bloom([0x01u8; 256]);
        let b = Bloom([0x01u8; 256]);
        assert_eq!(a, b);
    }

    #[test]
    fn add_needs_at_least_6_bytes() {
        let mut b = Bloom::new();
        b.add(&[1, 2, 3]);  // less than 6 — must not panic
        assert!(b.is_zero());
    }
}
