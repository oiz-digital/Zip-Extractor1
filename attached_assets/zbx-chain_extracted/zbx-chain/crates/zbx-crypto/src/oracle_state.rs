//! Task #5 (Precompile 0x0C — Price oracle read): shared state-layout helper.
//!
//! Both `zbx-zvm` and `zbx-evm` route the 0x0C precompile through this
//! module so the two execution engines cannot drift on the storage
//! layout used by the on-chain price registry. The corresponding
//! Solidity reference is the per-feed state in `ZbxAggregatorV3.sol`
//! consolidated into a registry mapping at the well-known address
//! [`ORACLE_REGISTRY_ADDRESS`].
//!
//! ABI:
//!   * Input  — exactly 32 bytes: `keccak256(asset_symbol)` (e.g.
//!     `keccak256("BTC/USD")`).
//!   * Output — exactly 64 bytes:
//!       * bytes  [0..32) — `int256 price_e8`   (big-endian, two's
//!         complement; matches `latestRoundData().answer`).
//!       * bytes [32..64) — `uint256 updated_at` (big-endian, unix
//!         seconds; matches `latestRoundData().updatedAt`).
//!   * Unknown asset — returns 64 zero bytes (no revert) so callers
//!     can branch.
//!   * Staleness check is the CALLER's responsibility — this
//!     precompile NEVER reverts on stale prices (per task spec).
//!   * Gas — `BASE_GAS + PER_SLOT_GAS * 2` (1000 + 200 = 1200).

use crate::keccak::keccak256_pair;

/// Well-known pseudo-system contract address that holds the price
/// registry mapping. Last byte is `0xCC` to mirror the precompile id
/// `0x0C` (system contracts are clustered at low addresses).
pub const ORACLE_REGISTRY_ADDRESS: [u8; 20] = {
    let mut a = [0u8; 20];
    a[19] = 0xCC;
    a
};

/// Solidity mapping slot index that holds `mapping(bytes32 => Feed)`
/// in the registry contract layout. Pinned to 0 so the layout is
/// trivially audit-able.
pub const FEED_MAP_SLOT: [u8; 32] = [0u8; 32];

/// Gas schedule (task spec).
pub const BASE_GAS: u64 = 1_000;
pub const PER_SLOT_GAS: u64 = 100;
pub const TOTAL_GAS: u64 = BASE_GAS + PER_SLOT_GAS * 2;

/// Errors returned by [`do_price_oracle`].
#[derive(Debug, PartialEq, Eq)]
pub enum OraclePrecompileError {
    /// Input was not exactly 32 bytes.
    BadInputLength { got: usize },
    /// Caller did not budget [`TOTAL_GAS`].
    OutOfGas,
}

impl core::fmt::Display for OraclePrecompileError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BadInputLength { got } => write!(f, "oracle: input must be 32 bytes, got {}", got),
            Self::OutOfGas => write!(f, "oracle: out of gas"),
        }
    }
}

/// Minimal state-read interface implemented by every host (ZVM + EVM).
/// Lives here so neither VM crate has to depend on the other.
pub trait OracleStateReader {
    /// Read the 32-byte storage slot at `(addr, slot)`. Must return
    /// `[0u8; 32]` for unset slots (matches EVM SLOAD semantics).
    fn read_slot(&self, addr: &[u8; 20], slot: &[u8; 32]) -> [u8; 32];
}

/// Compute the two storage slots that hold `(price_e8, updated_at)`
/// for `symbol_hash`. Mirrors Solidity:
///
/// ```text
/// struct Feed { int256 price; uint256 updatedAt; }
/// mapping(bytes32 => Feed) feeds;  // FEED_MAP_SLOT (slot 0)
/// // base = keccak256(symbol_hash || uint256(0))
/// // price slot     = base
/// // updated_at slot = base + 1
/// ```
pub fn slot_pair(symbol_hash: &[u8; 32]) -> ([u8; 32], [u8; 32]) {
    let base = keccak256_pair(symbol_hash, &FEED_MAP_SLOT);
    let mut base_arr = [0u8; 32];
    base_arr.copy_from_slice(base.as_bytes());
    // base + 1 (big-endian add).
    let mut next = base_arr;
    for byte in next.iter_mut().rev() {
        let (b, carry) = byte.overflowing_add(1);
        *byte = b;
        if !carry {
            break;
        }
    }
    (base_arr, next)
}

/// 0x0C precompile body. Shared between EVM and ZVM dispatchers; both
/// engines call this with their own host adapter so the two execution
/// paths produce byte-identical (output, gas) pairs.
pub fn do_price_oracle<R: OracleStateReader + ?Sized>(
    input: &[u8],
    gas_limit: u64,
    reader: &R,
) -> Result<(Vec<u8>, u64), OraclePrecompileError> {
    if input.len() != 32 {
        return Err(OraclePrecompileError::BadInputLength { got: input.len() });
    }
    if gas_limit < TOTAL_GAS {
        return Err(OraclePrecompileError::OutOfGas);
    }
    let mut symbol = [0u8; 32];
    symbol.copy_from_slice(input);
    let (price_slot, ts_slot) = slot_pair(&symbol);
    let price = reader.read_slot(&ORACLE_REGISTRY_ADDRESS, &price_slot);
    let ts    = reader.read_slot(&ORACLE_REGISTRY_ADDRESS, &ts_slot);
    let mut out = Vec::with_capacity(64);
    out.extend_from_slice(&price);
    out.extend_from_slice(&ts);
    Ok((out, TOTAL_GAS))
}

/// Convenience helper used by genesis-seed loaders: write `(price_e8,
/// updated_at)` for `symbol_hash` into a `(addr, slot) → value` writer.
pub fn write_feed<F>(
    symbol_hash: &[u8; 32],
    price_e8_be: [u8; 32],
    updated_at_be: [u8; 32],
    mut writer: F,
) where
    F: FnMut(&[u8; 20], [u8; 32], [u8; 32]),
{
    let (price_slot, ts_slot) = slot_pair(symbol_hash);
    writer(&ORACLE_REGISTRY_ADDRESS, price_slot, price_e8_be);
    writer(&ORACLE_REGISTRY_ADDRESS, ts_slot, updated_at_be);
}

/// Encode a positive `int256` price (e8) as 32 big-endian bytes.
/// Negative prices are not used by any current feed (Chainlink also
/// uses unsigned in practice) — callers needing them must encode
/// two's-complement themselves.
pub fn encode_price_e8(price_e8: u128) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[16..].copy_from_slice(&price_e8.to_be_bytes());
    out
}

/// Encode a unix timestamp as 32 big-endian bytes.
pub fn encode_timestamp(ts: u64) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[24..].copy_from_slice(&ts.to_be_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct MapReader(HashMap<([u8; 20], [u8; 32]), [u8; 32]>);
    impl OracleStateReader for MapReader {
        fn read_slot(&self, addr: &[u8; 20], slot: &[u8; 32]) -> [u8; 32] {
            self.0.get(&(*addr, *slot)).copied().unwrap_or([0u8; 32])
        }
    }

    fn sym(name: &str) -> [u8; 32] {
        let mut h = [0u8; 32];
        h.copy_from_slice(crate::keccak::keccak256(name.as_bytes()).as_bytes());
        h
    }

    fn seed(map: &mut HashMap<([u8; 20], [u8; 32]), [u8; 32]>, name: &str, p: u128, t: u64) {
        let s = sym(name);
        write_feed(&s, encode_price_e8(p), encode_timestamp(t), |a, slot, v| {
            map.insert((*a, slot), v);
        });
    }

    #[test]
    fn registered_feed_round_trip() {
        let mut m = HashMap::new();
        seed(&mut m, "BTC/USD", 67_500_00000000, 1_700_000_000);
        let r = MapReader(m);
        let s = sym("BTC/USD");
        let (out, gas) = do_price_oracle(&s, 100_000, &r).unwrap();
        assert_eq!(gas, TOTAL_GAS);
        assert_eq!(out.len(), 64);
        assert_eq!(out[0..32], encode_price_e8(67_500_00000000));
        assert_eq!(out[32..64], encode_timestamp(1_700_000_000));
    }

    #[test]
    fn unknown_feed_returns_zeros_no_revert() {
        let r = MapReader(HashMap::new());
        let s = sym("DOES/NOT/EXIST");
        let (out, gas) = do_price_oracle(&s, 100_000, &r).unwrap();
        assert_eq!(gas, TOTAL_GAS);
        assert_eq!(out, vec![0u8; 64]);
    }

    #[test]
    fn input_length_must_be_32() {
        let r = MapReader(HashMap::new());
        assert_eq!(
            do_price_oracle(&[0u8; 31], 100_000, &r),
            Err(OraclePrecompileError::BadInputLength { got: 31 }),
        );
        assert_eq!(
            do_price_oracle(&[0u8; 33], 100_000, &r),
            Err(OraclePrecompileError::BadInputLength { got: 33 }),
        );
    }

    #[test]
    fn out_of_gas_below_total() {
        let r = MapReader(HashMap::new());
        assert_eq!(
            do_price_oracle(&[0u8; 32], TOTAL_GAS - 1, &r),
            Err(OraclePrecompileError::OutOfGas),
        );
        // exact budget passes
        assert!(do_price_oracle(&[0u8; 32], TOTAL_GAS, &r).is_ok());
    }

    #[test]
    fn distinct_symbols_distinct_slots() {
        let (p_a, t_a) = slot_pair(&sym("BTC/USD"));
        let (p_b, t_b) = slot_pair(&sym("ETH/USD"));
        assert_ne!(p_a, p_b);
        assert_ne!(t_a, t_b);
        // ts slot is exactly base + 1.
        let mut bumped = p_a;
        for b in bumped.iter_mut().rev() {
            let (n, c) = b.overflowing_add(1);
            *b = n;
            if !c { break; }
        }
        assert_eq!(bumped, t_a);
    }

    #[test]
    fn registry_address_last_byte_is_cc() {
        assert_eq!(ORACLE_REGISTRY_ADDRESS[19], 0xCC);
        assert!(ORACLE_REGISTRY_ADDRESS[..19].iter().all(|&b| b == 0));
    }

    #[test]
    fn five_seeded_feeds_independent() {
        let mut m = HashMap::new();
        seed(&mut m, "ZBX/USD",  2_50_000_000,           1_700_000_000); // $2.50
        seed(&mut m, "ETH/USD",  3_500_00_000_000,       1_700_000_001); // $3500
        seed(&mut m, "BTC/USD",  67_500_00_000_000,      1_700_000_002); // $67500
        seed(&mut m, "USDT/USD", 1_00_000_000,           1_700_000_003); // $1.00
        seed(&mut m, "USDC/USD", 99_950_000,             1_700_000_004); // $0.9995
        let r = MapReader(m);
        for (name, p, t) in [
            ("ZBX/USD",  2_50_000_000u128,      1_700_000_000u64),
            ("ETH/USD",  3_500_00_000_000,      1_700_000_001),
            ("BTC/USD",  67_500_00_000_000,     1_700_000_002),
            ("USDT/USD", 1_00_000_000,          1_700_000_003),
            ("USDC/USD", 99_950_000,            1_700_000_004),
        ] {
            let (out, _) = do_price_oracle(&sym(name), 100_000, &r).unwrap();
            assert_eq!(out[0..32], encode_price_e8(p), "{name} price");
            assert_eq!(out[32..64], encode_timestamp(t), "{name} ts");
        }
    }
}
