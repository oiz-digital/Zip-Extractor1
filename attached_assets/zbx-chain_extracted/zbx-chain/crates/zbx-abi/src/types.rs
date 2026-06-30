//! ABI type system.

use serde::{Deserialize, Serialize};

/// A Solidity ABI type tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AbiType {
    // --- Integers ---
    Uint(u16),           // uint8 .. uint256 (must be multiple of 8)
    Int(u16),            // int8  .. int256

    // --- Fixed-size ---
    Bool,
    Address,
    /// `bytesN` for N in 1..=32 (fixed-size byte string).
    FixedBytes(u8),
    FixedArray(Box<AbiType>, usize),

    // --- Dynamic ---
    /// `bytes` (variable-length byte string).
    Bytes,
    String,
    Array(Box<AbiType>),
    Tuple(Vec<AbiType>),
}

impl AbiType {
    /// Whether this type is dynamically-sized (requires offset pointer).
    pub fn is_dynamic(&self) -> bool {
        matches!(
            self,
            AbiType::Bytes | AbiType::String | AbiType::Array(_)
                | AbiType::FixedArray(_, _)
                | AbiType::Tuple(_)
        ) || matches!(self, AbiType::FixedArray(t, _) if t.is_dynamic())
           || matches!(self, AbiType::Tuple(ts) if ts.iter().any(|t| t.is_dynamic()))
    }

    /// Head size in bytes (32 for dynamic, actual for static).
    pub fn head_size(&self) -> usize {
        32
    }

    /// Parse a Solidity type string.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        Some(match s {
            "bool"    => AbiType::Bool,
            "address" => AbiType::Address,
            "bytes"   => AbiType::Bytes,
            "string"  => AbiType::String,
            _ if s.starts_with("uint") => {
                let bits: u16 = s[4..].parse().ok().unwrap_or(256);
                AbiType::Uint(bits)
            }
            _ if s.starts_with("int") => {
                let bits: u16 = s[3..].parse().ok().unwrap_or(256);
                AbiType::Int(bits)
            }
            _ if s.starts_with("bytes") && !s.contains('[') => {
                let n: u8 = s[5..].parse().ok()?;
                AbiType::FixedBytes(n)
            }
            _ if s.ends_with("[]") => {
                let inner = AbiType::parse(&s[..s.len()-2])?;
                AbiType::Array(Box::new(inner))
            }
            _ => return None,
        })
    }

    /// Canonical type string.
    pub fn canonical(&self) -> String {
        match self {
            AbiType::Uint(n)  => format!("uint{}", n),
            AbiType::Int(n)   => format!("int{}", n),
            AbiType::Bool     => "bool".to_string(),
            AbiType::Address  => "address".to_string(),
            AbiType::Bytes        => "bytes".to_string(),
            AbiType::String       => "string".to_string(),
            AbiType::FixedBytes(n) => format!("bytes{}", n),
            AbiType::Array(t) => format!("{}[]", t.canonical()),
            AbiType::FixedArray(t, n) => format!("{}[{}]", t.canonical(), n),
            AbiType::Tuple(ts) => {
                let inner: Vec<_> = ts.iter().map(|t| t.canonical()).collect();
                format!("({})", inner.join(","))
            }
        }
    }
}

/// A decoded ABI value.
#[derive(Debug, Clone, PartialEq)]
pub enum AbiValue {
    Uint(u128),
    Int(i128),
    Bool(bool),
    Address([u8; 20]),
    FixedBytes(Vec<u8>),
    Bytes(Vec<u8>),
    String(String),
    Array(Vec<AbiValue>),
    Tuple(Vec<AbiValue>),
}

impl AbiValue {
    pub fn as_uint(&self) -> Option<u128> {
        if let Self::Uint(n) = self { Some(*n) } else { None }
    }

    pub fn as_address(&self) -> Option<[u8; 20]> {
        if let Self::Address(a) = self { Some(*a) } else { None }
    }

    pub fn as_bool(&self) -> Option<bool> {
        if let Self::Bool(b) = self { Some(*b) } else { None }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_primitives() {
        assert_eq!(AbiType::parse("bool"),    Some(AbiType::Bool));
        assert_eq!(AbiType::parse("address"), Some(AbiType::Address));
        assert_eq!(AbiType::parse("bytes"),   Some(AbiType::Bytes));
        assert_eq!(AbiType::parse("string"),  Some(AbiType::String));
    }

    #[test]
    fn parse_uint_and_int() {
        assert_eq!(AbiType::parse("uint256"), Some(AbiType::Uint(256)));
        assert_eq!(AbiType::parse("uint8"),   Some(AbiType::Uint(8)));
        assert_eq!(AbiType::parse("int128"),  Some(AbiType::Int(128)));
    }

    #[test]
    fn parse_bytes_fixed() {
        assert_eq!(AbiType::parse("bytes32"), Some(AbiType::FixedBytes(32)));
        assert_eq!(AbiType::parse("bytes1"),  Some(AbiType::FixedBytes(1)));
    }

    #[test]
    fn parse_dynamic_array() {
        let t = AbiType::parse("uint256[]").unwrap();
        assert_eq!(t, AbiType::Array(Box::new(AbiType::Uint(256))));
        assert!(t.is_dynamic());
    }

    #[test]
    fn parse_unknown_returns_none() {
        assert_eq!(AbiType::parse("mapping(address=>uint)"), None);
    }

    #[test]
    fn canonical_roundtrip() {
        for t in [
            AbiType::Bool,
            AbiType::Address,
            AbiType::Uint(256),
            AbiType::Int(128),
            AbiType::FixedBytes(32),
            AbiType::Bytes,
            AbiType::String,
        ] {
            let canon = t.canonical();
            let parsed = AbiType::parse(&canon).unwrap();
            assert_eq!(parsed, t, "failed for {}", canon);
        }
    }

    #[test]
    fn is_dynamic_static_types() {
        assert!(!AbiType::Bool.is_dynamic());
        assert!(!AbiType::Address.is_dynamic());
        assert!(!AbiType::Uint(256).is_dynamic());
        assert!(!AbiType::FixedBytes(32).is_dynamic());
    }

    #[test]
    fn is_dynamic_dynamic_types() {
        assert!(AbiType::Bytes.is_dynamic());
        assert!(AbiType::String.is_dynamic());
        assert!(AbiType::Array(Box::new(AbiType::Uint(256))).is_dynamic());
    }

    #[test]
    fn head_size_is_always_32() {
        assert_eq!(AbiType::Bool.head_size(), 32);
        assert_eq!(AbiType::Bytes.head_size(), 32);
        assert_eq!(AbiType::Tuple(vec![AbiType::Uint(256)]).head_size(), 32);
    }

    #[test]
    fn tuple_canonical() {
        let t = AbiType::Tuple(vec![AbiType::Uint(256), AbiType::Address]);
        assert_eq!(t.canonical(), "(uint256,address)");
    }
}
