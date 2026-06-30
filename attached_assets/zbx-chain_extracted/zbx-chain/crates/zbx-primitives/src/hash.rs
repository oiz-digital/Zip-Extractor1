//! 32-byte / 20-byte hash newtype wrappers.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct H256(pub [u8; 32]);

impl H256 {
    pub const ZERO: Self = Self([0u8; 32]);

    pub fn from_slice(s: &[u8]) -> Self {
        let mut b = [0u8; 32];
        b.copy_from_slice(s);
        Self(b)
    }

    pub fn as_bytes(&self) -> &[u8; 32] { &self.0 }
    pub fn to_bytes(self) -> [u8; 32] { self.0 }
    pub fn is_zero(&self) -> bool { self.0 == [0u8; 32] }
}

impl From<[u8; 32]> for H256 { fn from(v: [u8; 32]) -> Self { Self(v) } }
impl From<H256> for [u8; 32] { fn from(v: H256) -> Self { v.0 } }
impl AsRef<[u8]> for H256 { fn as_ref(&self) -> &[u8] { &self.0 } }

impl fmt::Display for H256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x")?;
        for b in &self.0 { write!(f, "{:02x}", b)?; }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct H160(pub [u8; 20]);

impl H160 {
    pub const ZERO: Self = Self([0u8; 20]);

    pub fn from_slice(s: &[u8]) -> Self {
        let mut b = [0u8; 20];
        b.copy_from_slice(s);
        Self(b)
    }

    pub fn as_bytes(&self) -> &[u8; 20] { &self.0 }
    pub fn to_bytes(self) -> [u8; 20] { self.0 }
    pub fn is_zero(&self) -> bool { self.0 == [0u8; 20] }
}

impl From<[u8; 20]> for H160 { fn from(v: [u8; 20]) -> Self { Self(v) } }
impl From<H160> for [u8; 20] { fn from(v: H160) -> Self { v.0 } }
impl AsRef<[u8]> for H160 { fn as_ref(&self) -> &[u8] { &self.0 } }

impl fmt::Display for H160 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x")?;
        for b in &self.0 { write!(f, "{:02x}", b)?; }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn h256_zero_is_zero() {
        assert!(H256::ZERO.is_zero());
        assert!(!H256([1u8; 32]).is_zero());
    }

    #[test]
    fn h256_from_slice_roundtrip() {
        let raw = [0xabu8; 32];
        let h = H256::from_slice(&raw);
        assert_eq!(h.as_bytes(), &raw);
        assert_eq!(<[u8;32]>::from(h), raw);
    }

    #[test]
    fn h256_into_from_array() {
        let arr = [0x01u8; 32];
        let h: H256 = arr.into();
        let back: [u8; 32] = h.into();
        assert_eq!(back, arr);
    }

    #[test]
    fn h256_display_is_hex() {
        let h = H256([0xffu8; 32]);
        assert!(format!("{}", h).starts_with("0x"));
        assert_eq!(format!("{}", h).len(), 66);
    }

    #[test]
    fn h160_zero_is_zero() {
        assert!(H160::ZERO.is_zero());
    }

    #[test]
    fn h160_from_slice_roundtrip() {
        let raw = [0x42u8; 20];
        let h = H160::from_slice(&raw);
        assert_eq!(h.to_bytes(), raw);
    }

    #[test]
    fn h256_as_ref_is_slice() {
        let h = H256([0x55u8; 32]);
        let s: &[u8] = h.as_ref();
        assert_eq!(s.len(), 32);
        assert!(s.iter().all(|&b| b == 0x55));
    }
}
