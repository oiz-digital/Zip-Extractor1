//! EVM-compatible 20-byte address with checksum encoding (EIP-55).

use crate::error::ZbxError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha3::{Digest, Keccak256};
use std::fmt;
use std::str::FromStr;

/// A 20-byte Ethereum-compatible account address.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, PartialOrd, Ord)]
pub struct Address(pub [u8; 20]);

impl Address {
    /// The zero address (0x0000...0000).
    pub const ZERO: Self = Address([0u8; 20]);

    /// Convenience alias for [`Address::ZERO`] used by EVM call sites that
    /// follow the `primitive_types::H160::zero()` naming convention.
    pub const fn zero() -> Self {
        Self::ZERO
    }

    /// Construct from raw bytes.
    pub fn from_bytes(b: &[u8]) -> Result<Self, ZbxError> {
        if b.len() != 20 {
            return Err(ZbxError::InvalidLength { expected: 20, got: b.len() });
        }
        let mut arr = [0u8; 20];
        arr.copy_from_slice(b);
        Ok(Address(arr))
    }

    /// Decode from a hex string with or without 0x prefix.
    pub fn from_hex(s: &str) -> Result<Self, ZbxError> {
        let s = s.strip_prefix("0x").unwrap_or(s);
        let b = hex::decode(s).map_err(|_| ZbxError::InvalidHex(s.to_string()))?;
        Self::from_bytes(&b)
    }

    /// Raw bytes.
    pub fn as_bytes(&self) -> &[u8; 20] {
        &self.0
    }

    /// EIP-55 checksummed hex representation.
    pub fn to_checksum(&self) -> String {
        let hex = hex::encode(self.0);
        let hash = Keccak256::digest(hex.as_bytes());
        let hash_hex = hex::encode(hash);
        let checksum: String = hex
            .chars()
            .zip(hash_hex.chars())
            .map(|(c, h)| {
                if c.is_alphabetic() && h >= '8' {
                    c.to_ascii_uppercase()
                } else {
                    c
                }
            })
            .collect();
        format!("0x{}", checksum)
    }

    /// Pad to 32 bytes (left-zero-padded) for ABI encoding.
    pub fn to_h256(&self) -> [u8; 32] {
        let mut out = [0u8; 32];
        out[12..].copy_from_slice(&self.0);
        out
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_checksum())
    }
}

impl fmt::Debug for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Address({})", self.to_checksum())
    }
}

impl fmt::LowerHex for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{}", hex::encode(self.0))
    }
}

impl FromStr for Address {
    type Err = ZbxError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_hex(s)
    }
}

impl Serialize for Address {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_checksum())
    }
}

impl<'de> Deserialize<'de> for Address {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::from_hex(&s).map_err(serde::de::Error::custom)
    }
}