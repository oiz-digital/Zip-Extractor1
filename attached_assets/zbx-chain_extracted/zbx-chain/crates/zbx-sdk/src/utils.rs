//! Utility functions: formatting, parsing, address checksums.

use zbx_types::{Address, U256};
use crate::signer::keccak256;

// ── ZBX formatting ────────────────────────────────────────────────────────────

/// Format a Wei (U256) amount as a human-readable ZBX string.
///
/// ```
/// use zbx_sdk::utils::format_zbx;
/// use zbx_types::U256;
/// assert_eq!(format_zbx(U256::from(1_500_000_000_000_000_000u128)), "1.5 ZBX");
/// ```
pub fn format_zbx(wei: U256) -> String {
    const ETH: u128 = 1_000_000_000_000_000_000u128;
    let whole = wei.as_u128() / ETH;
    let frac  = wei.as_u128() % ETH;
    if frac == 0 {
        format!("{} ZBX", whole)
    } else {
        let frac_str = format!("{:018}", frac);
        let trimmed  = frac_str.trim_end_matches('0');
        format!("{}.{} ZBX", whole, trimmed)
    }
}

/// Parse a ZBX decimal string (e.g. "1.5" or "100") to Wei (U256).
///
/// ```
/// use zbx_sdk::utils::parse_zbx;
/// use zbx_types::U256;
/// assert_eq!(parse_zbx("1.5").unwrap(), U256::from(1_500_000_000_000_000_000u128));
/// ```
pub fn parse_zbx(amount: &str) -> Option<U256> {
    let amount = amount.trim();
    let parts: Vec<&str> = amount.splitn(2, '.').collect();
    let whole: u128 = parts[0].parse().ok()?;
    let mut wei = whole.checked_mul(1_000_000_000_000_000_000u128)?;
    if parts.len() == 2 {
        let frac_str = format!("{:0<18}", parts[1]);
        let frac_trimmed = &frac_str[..frac_str.len().min(18)];
        let frac: u128 = frac_trimmed.parse().ok()?;
        wei = wei.checked_add(frac)?;
    }
    Some(U256::from(wei))
}

/// Format a Wei amount as a Gwei string.
pub fn format_gwei(wei: U256) -> String {
    let gwei = wei.as_u128() as f64 / 1e9;
    format!("{:.2} gwei", gwei)
}

// ── Address helpers ────────────────────────────────────────────────────────────

/// Convert an address to EIP-55 checksum format.
///
/// ```
/// use zbx_sdk::utils::to_checksum;
/// use zbx_types::Address;
/// let addr = Address([0x5a; 20]);
/// assert!(to_checksum(addr).starts_with("0x"));
/// ```
pub fn to_checksum(addr: Address) -> String {
    let hex    = hex::encode(addr.as_bytes());
    let hash   = keccak256(hex.as_bytes());
    let mut out = String::with_capacity(42);
    out.push_str("0x");
    for (i, ch) in hex.chars().enumerate() {
        let nibble = (hash.as_bytes()[i / 2] >> if i % 2 == 0 { 4 } else { 0 }) & 0xf;
        if nibble >= 8 { out.push(ch.to_ascii_uppercase()); }
        else           { out.push(ch.to_ascii_lowercase()); }
    }
    out
}

/// Parse a hex address string (with or without 0x prefix) to `Address`.
pub fn parse_address(s: &str) -> Option<Address> {
    let clean = s.trim_start_matches("0x");
    let bytes  = hex::decode(clean).ok()?;
    if bytes.len() != 20 { return None; }
    let mut arr = [0u8; 20];
    arr.copy_from_slice(&bytes);
    Some(Address(arr))
}

/// Parse a hex H256 hash string to `H256`.
pub fn parse_hash(s: &str) -> Option<zbx_types::H256> {
    let clean = s.trim_start_matches("0x");
    let bytes  = hex::decode(clean).ok()?;
    if bytes.len() != 32 { return None; }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Some(zbx_types::H256(arr))
}

pub fn is_zero_address(addr: Address) -> bool {
    addr.as_bytes().iter().all(|&b| b == 0)
}

// ── Block helpers ─────────────────────────────────────────────────────────────

/// Convert a block number to a hex string for RPC calls.
pub fn block_tag(n: u64) -> String { format!("0x{:x}", n) }

/// Convert wei to gwei (f64).
pub fn wei_to_gwei(wei: U256) -> f64 { wei.as_u128() as f64 / 1e9 }

/// Convert gwei to wei.
pub fn gwei_to_wei(gwei: f64) -> U256 { U256::from((gwei * 1e9) as u128) }

/// Convert ether to wei.
pub fn eth_to_wei(eth: f64) -> U256 { U256::from((eth * 1e18) as u128) }

/// Convert wei to ether (f64, loses precision for large values).
pub fn wei_to_eth(wei: U256) -> f64 { wei.as_u128() as f64 / 1e18 }

#[cfg(test)]
mod tests {
    use super::*;
    use zbx_types::U256;

    #[test]
    fn test_format_zbx_whole() {
        assert_eq!(format_zbx(U256::from(1_000_000_000_000_000_000u128)), "1 ZBX");
    }

    #[test]
    fn test_format_zbx_fractional() {
        assert_eq!(format_zbx(U256::from(1_500_000_000_000_000_000u128)), "1.5 ZBX");
    }

    #[test]
    fn test_parse_zbx_whole() {
        assert_eq!(parse_zbx("1").unwrap(), U256::from(1_000_000_000_000_000_000u128));
    }

    #[test]
    fn test_parse_zbx_fractional() {
        assert_eq!(parse_zbx("1.5").unwrap(), U256::from(1_500_000_000_000_000_000u128));
    }

    #[test]
    fn test_roundtrip() {
        let original = U256::from(12_345_678_000_000_000_000u128);
        let formatted = format_zbx(original);
        let parsed = parse_zbx(formatted.trim_end_matches(" ZBX")).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn test_wei_gwei_conversion() {
        let one_gwei = gwei_to_wei(1.0);
        assert_eq!(one_gwei, U256::from(1_000_000_000u64));
        assert!((wei_to_gwei(one_gwei) - 1.0).abs() < 1e-9);
    }
}