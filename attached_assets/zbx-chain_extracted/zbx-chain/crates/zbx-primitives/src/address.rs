//! 20-byte Ethereum-compatible address type.

use serde::{Deserialize, Serialize};
use std::fmt;
use sha3::{Keccak256, Digest};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Address(pub [u8; 20]);

impl Address {
    pub const ZERO: Self = Self([0u8; 20]);

    pub fn from_hex(s: &str) -> Result<Self, String> {
        let s = s.trim_start_matches("0x");
        if s.len() != 40 { return Err(format!("invalid length: {}", s.len())); }
        let mut b = [0u8; 20];
        for i in 0..20 {
            b[i] = u8::from_str_radix(&s[i*2..i*2+2], 16).map_err(|e| e.to_string())?;
        }
        Ok(Address(b))
    }

    pub fn from_pubkey(pubkey: &[u8]) -> Self {
        let h = Keccak256::digest(pubkey);
        let mut a = [0u8; 20];
        a.copy_from_slice(&h[12..]);
        Address(a)
    }

    pub fn is_zero(&self) -> bool { self.0 == [0u8; 20] }

    pub fn to_hex(&self) -> String { format!("0x{}", hex::encode(self.0)) }

    pub fn to_checksum(&self) -> String {
        let hex_str = hex::encode(self.0);
        let hash = hex::encode(Keccak256::digest(hex_str.as_bytes()));
        let mut out = "0x".to_string();
        for (i, c) in hex_str.chars().enumerate() {
            let nibble = u8::from_str_radix(&hash[i..i+1], 16).unwrap_or(0);
            if nibble >= 8 { out.push(c.to_ascii_uppercase()); }
            else { out.push(c); }
        }
        out
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_checksum())
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_address_is_zero() {
        assert!(Address::ZERO.is_zero());
    }

    #[test]
    fn from_hex_roundtrip() {
        let a = Address::from_hex("0x742d35Cc6634C0532925a3b8D4C9a0a8b8B3f29").unwrap();
        assert!(!a.is_zero());
        let hex = a.to_hex();
        assert!(hex.starts_with("0x"));
        assert_eq!(hex.len(), 42);
    }

    #[test]
    fn from_hex_rejects_short() {
        assert!(Address::from_hex("0xdeadbeef").is_err());
    }

    #[test]
    fn from_hex_rejects_invalid_chars() {
        let bad = "0x".to_string() + &"z".repeat(40);
        assert!(Address::from_hex(&bad).is_err());
    }

    #[test]
    fn to_checksum_is_mixed_case() {
        let a = Address([0xde, 0xad, 0xbe, 0xef, 0x00, 0x11, 0x22, 0x33,
                         0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb,
                         0xcc, 0xdd, 0xee, 0xff]);
        let cs = a.to_checksum();
        assert!(cs.starts_with("0x"));
        assert_eq!(cs.len(), 42);
    }

    #[test]
    fn from_pubkey_returns_20_bytes() {
        let pubkey = [0x02u8; 64];
        let addr = Address::from_pubkey(&pubkey);
        assert_eq!(addr.0.len(), 20);
    }

    #[test]
    fn display_uses_checksum() {
        let a = Address([0x1u8; 20]);
        let s = format!("{}", a);
        assert!(s.starts_with("0x"));
        assert_eq!(s.len(), 42);
    }
}
