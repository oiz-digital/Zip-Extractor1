//! Task #7 (Precompile 0x0F — ZUSD vault state direct-read): shared body.
//!
//! Both `zbx-zvm` and `zbx-evm` route the 0x0F precompile through this
//! module so the two execution engines cannot drift on the storage
//! layout used by the on-chain ZUSD vault registry. The Solidity
//! reference is `contracts/ZbxVaultRegistry.sol` — a deliberately
//! minimal, no-inheritance contract whose `mapping(address => CDP)
//! cdps` is pinned at slot 0 (and proven by the contract's own
//! `CDPS_MAP_SLOT()` view). Pre-Pass-15 the precompile pointed at the
//! legacy `ZusdVault.sol` whose `cdps` slot drifted with inheritance
//! changes (Ownable2Step + ReentrancyGuard + state vars above the
//! mapping); the registry split removes that consensus risk.
//!
//! ABI:
//!   * Input  — exactly 32 bytes: a Solidity-padded `address` (12 zero
//!     bytes ‖ 20-byte vault owner).
//!   * Output — exactly 128 bytes (four big-endian uint256 fields):
//!       * bytes  [0..32)   — `uint256 collateral_zbx`     (wei, 1e18)
//!       * bytes [32..64)   — `uint256 debt_zusd`          (wei, 1e18)
//!       * bytes [64..96)   — `uint256 c_ratio_bps`        (10000 = 100%)
//!       * bytes [96..128)  — `uint256 liquidation_price_e18`
//!   * Non-existent / empty vault → 128 zero bytes (no revert).
//!   * Pass-15 OracleFreshness gate: when the host exposes a non-zero
//!     `current_timestamp` (production path), an oracle reading older
//!     than [`MAX_ORACLE_STALENESS`] seconds zeroes out the derived
//!     `c_ratio_bps` and `liquidation_price_e18` fields — raw
//!     `collateral_zbx` and `debt_zusd` still flow through. When the
//!     host reports `0` (legacy / test path), the gate is skipped.
//!     The precompile NEVER reverts on stale prices (per task spec —
//!     the caller chooses the policy).
//!   * Gas — `BASE_GAS + PER_SLOT_GAS * 4` (1500 + 200·4 = 2300). Two
//!     CDP slots (collateral, debt) plus two oracle slots
//!     (price_e8, updated_at) — the lastFeeIndex / openedAt CDP slots
//!     are NOT read for the four-field response.
//!
//! Storage layout (consensus-critical, pinned here):
//!   * Vault registry address = [`ZUSD_VAULT_ADDRESS`] (`0x..5455`).
//!   * `cdps` mapping slot    = [`CDPS_MAP_SLOT`] (slot 0, see
//!     `ZbxVaultRegistry.sol::CDPS_MAP_SLOT()`).
//!   * Per-CDP base = `keccak256(uint256(owner) ‖ CDPS_MAP_SLOT)`.
//!     * `collateral` = base + 0
//!     * `debt`       = base + 1
//!   * Price feed = `keccak256("ZBX/USD")` in the Task #5 oracle registry.
//!
//! Genesis contract: [`assert_vault_deployed`] is the helper a node
//! genesis loader MUST call at startup to assert that
//! `ZbxVaultRegistry.sol` is deployed at [`ZUSD_VAULT_ADDRESS`].
//! `node/configs/{testnet,mainnet}.toml` pin the contract artifact
//! that satisfies this check.

use crate::keccak::{keccak256, keccak256_pair};
use crate::oracle_state::{slot_pair as oracle_slot_pair, ORACLE_REGISTRY_ADDRESS};
use primitive_types::U256;

/// Canonical address of the ZUSD vault registry contract. Last two
/// bytes are `0x54 0x55` (= `0x5455`) per CHAIN_CONSTANTS.
pub const ZUSD_VAULT_ADDRESS: [u8; 20] = {
    let mut a = [0u8; 20];
    a[18] = 0x54;
    a[19] = 0x55;
    a
};

/// Solidity slot index of the `cdps` mapping inside `ZbxVaultRegistry`.
/// Pinned to 0; the contract is intentionally inheritance-free so this
/// slot is invariant across compiler versions and is asserted by the
/// contract's own `CDPS_MAP_SLOT()` pure view.
pub const CDPS_MAP_SLOT: [u8; 32] = [0u8; 32];

/// Symbol the oracle price is read under for the c-ratio + liquidation
/// price computation. Matches the Task #5 oracle keying convention
/// (`keccak256(symbol)`).
pub const ZBX_USD_FEED_SYMBOL: &[u8] = b"ZBX/USD";

/// Gas schedule (task spec).
pub const BASE_GAS: u64 = 1_500;
pub const PER_SLOT_GAS: u64 = 200;
pub const SLOTS_READ: u64 = 4; // collateral, debt, oracle price, oracle ts
pub const TOTAL_GAS: u64 = BASE_GAS + PER_SLOT_GAS * SLOTS_READ;

/// `100 %` in basis points.
pub const BPS_DENOM: u64 = 10_000;

/// Pass-15 OracleFreshness window (seconds). Mirrors the Solidity
/// `MAX_STALENESS` default in `OracleFreshness.sol` and the legacy
/// `ZusdVault._freshPrice` helper. The precompile only enforces this
/// gate when the host exposes a non-zero current timestamp
/// (`VaultStateReader::current_timestamp`).
pub const MAX_ORACLE_STALENESS: u64 = 3_600;

/// Errors returned by [`do_zusd_vault_read`].
#[derive(Debug, PartialEq, Eq)]
pub enum VaultPrecompileError {
    /// Input was not exactly 32 bytes.
    BadInputLength { got: usize },
    /// Caller did not budget [`TOTAL_GAS`].
    OutOfGas,
}

impl core::fmt::Display for VaultPrecompileError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BadInputLength { got } => {
                write!(f, "vault: input must be 32 bytes (padded address), got {}", got)
            }
            Self::OutOfGas => write!(f, "vault: out of gas"),
        }
    }
}

/// Read-only host surface required by the 0x0F precompile.
///
/// `read_slot` mirrors `OracleStateReader` (same signature) so the same
/// host adapter that backs Task #5's 0x0C also satisfies this trait.
/// `current_timestamp` is the Pass-15 OracleFreshness hook: non-zero
/// returns enable staleness gating, `0` (the default) skips it.
pub trait VaultStateReader {
    fn read_slot(&self, addr: &[u8; 20], slot: &[u8; 32]) -> [u8; 32];
    /// Block timestamp at the moment of the read. Default `0` ⇒
    /// freshness check disabled (legacy / test).
    fn current_timestamp(&self) -> u64 {
        0
    }
}

/// Compute the two storage slots that hold `(collateral, debt)` for a
/// given vault owner. Mirrors Solidity:
///
/// ```text
/// struct CDP { uint256 collateral; uint256 debt; uint256 lastFeeIndex; uint256 openedAt; }
/// mapping(address => CDP) cdps;  // slot CDPS_MAP_SLOT
/// // base               = keccak256(uint256(owner) || CDPS_MAP_SLOT)
/// // collateral_slot    = base + 0
/// // debt_slot          = base + 1
/// ```
pub fn cdp_slots(owner: &[u8; 20]) -> ([u8; 32], [u8; 32]) {
    let mut owner_padded = [0u8; 32];
    owner_padded[12..].copy_from_slice(owner);
    let base = keccak256_pair(&owner_padded, &CDPS_MAP_SLOT);
    let mut collateral_slot = [0u8; 32];
    collateral_slot.copy_from_slice(base.as_bytes());
    let mut debt_slot = collateral_slot;
    add_one_be(&mut debt_slot);
    (collateral_slot, debt_slot)
}

/// Big-endian `+= 1` with wrap (the wrap is unreachable for a real
/// keccak256 output but kept total to avoid panic paths).
fn add_one_be(b: &mut [u8; 32]) {
    for byte in b.iter_mut().rev() {
        let (v, carry) = byte.overflowing_add(1);
        *byte = v;
        if !carry {
            return;
        }
    }
}

/// 0x0F precompile body. Shared between EVM and ZVM dispatchers; both
/// engines call this with their own host adapter so the two execution
/// paths produce byte-identical (output, gas) pairs.
pub fn do_zusd_vault_read<R: VaultStateReader + ?Sized>(
    input: &[u8],
    gas_limit: u64,
    reader: &R,
) -> Result<(Vec<u8>, u64), VaultPrecompileError> {
    if input.len() != 32 {
        return Err(VaultPrecompileError::BadInputLength { got: input.len() });
    }
    if gas_limit < TOTAL_GAS {
        return Err(VaultPrecompileError::OutOfGas);
    }

    // Solidity-padded address: 12 leading zero bytes, 20-byte address.
    let mut owner = [0u8; 20];
    owner.copy_from_slice(&input[12..32]);

    // ── 1) CDP slots ─────────────────────────────────────────────────
    let (collateral_slot, debt_slot) = cdp_slots(&owner);
    let collateral_be = reader.read_slot(&ZUSD_VAULT_ADDRESS, &collateral_slot);
    let debt_be = reader.read_slot(&ZUSD_VAULT_ADDRESS, &debt_slot);

    // Empty vault — return 128 zero bytes (no revert).
    if collateral_be == [0u8; 32] && debt_be == [0u8; 32] {
        return Ok((vec![0u8; 128], TOTAL_GAS));
    }

    // ── 2) Oracle price + timestamp (ZBX/USD) ────────────────────────
    let symbol_hash = {
        let mut h = [0u8; 32];
        h.copy_from_slice(keccak256(ZBX_USD_FEED_SYMBOL).as_bytes());
        h
    };
    let (price_slot, ts_slot) = oracle_slot_pair(&symbol_hash);
    let price_e8_be = reader.read_slot(&ORACLE_REGISTRY_ADDRESS, &price_slot);
    let ts_be = reader.read_slot(&ORACLE_REGISTRY_ADDRESS, &ts_slot);

    // ── 3) Pass-15 OracleFreshness gate ──────────────────────────────
    // When the host reports a real wall-clock time (non-zero), refuse
    // to derive c_ratio / liq_price from a stale price. We still pass
    // the raw collateral / debt through so callers can branch on the
    // empty derived fields.
    let now = reader.current_timestamp();
    let oracle_ts = u256_lower_u64(&ts_be);
    let stale = now > 0 && now.saturating_sub(oracle_ts) > MAX_ORACLE_STALENESS;

    // ── 4) Derived fields ────────────────────────────────────────────
    let collateral = U256::from_big_endian(&collateral_be);
    let debt = U256::from_big_endian(&debt_be);
    let price_e8 = U256::from_big_endian(&price_e8_be);

    let one_e18 = U256::from(1_000_000_000_000_000_000u64);
    let one_e10 = U256::from(10_000_000_000u64);
    let bps = U256::from(BPS_DENOM);

    let mut c_ratio_bps = U256::zero();
    let mut liq_price_e18 = U256::zero();

    if !stale && !price_e8.is_zero() && !debt.is_zero() && !collateral.is_zero() {
        // c_ratio_bps = collateral * price_e18 / 1e18 * 10000 / debt
        //             = collateral * price_e8 * 1e10 * 10000 / 1e18 / debt
        let price_e18 = price_e8.saturating_mul(one_e10);
        let collateral_value_e18 = collateral
            .checked_mul(price_e18)
            .map(|v| v / one_e18)
            .unwrap_or(U256::MAX);
        c_ratio_bps = collateral_value_e18
            .checked_mul(bps)
            .map(|v| v / debt)
            .unwrap_or(U256::MAX);

        // liq_price_e18 = debt * 1e18 / collateral
        liq_price_e18 = debt
            .checked_mul(one_e18)
            .map(|v| v / collateral)
            .unwrap_or(U256::MAX);
    }

    // ── 5) Pack 4 × uint256 BE ──────────────────────────────────────
    let mut out = vec![0u8; 128];
    out[0..32].copy_from_slice(&collateral_be);
    out[32..64].copy_from_slice(&debt_be);
    c_ratio_bps.to_big_endian(&mut out[64..96]);
    liq_price_e18.to_big_endian(&mut out[96..128]);

    Ok((out, TOTAL_GAS))
}

/// Read a u64 timestamp out of the lower 8 BE bytes of a 32-byte
/// Solidity uint256 slot. Higher bytes are ignored (defensive — a
/// real oracle never writes >2^64 there, but a malicious writer with
/// state access shouldn't be able to wrap us into a freshness bypass).
fn u256_lower_u64(be: &[u8; 32]) -> u64 {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&be[24..32]);
    u64::from_be_bytes(bytes)
}

/// Convenience helper used by tests + genesis seeders to write a CDP
/// `(collateral, debt)` pair into a `(addr, slot) → value` writer.
pub fn write_cdp<F>(owner: &[u8; 20], collateral_be: [u8; 32], debt_be: [u8; 32], mut writer: F)
where
    F: FnMut(&[u8; 20], [u8; 32], [u8; 32]),
{
    let (c_slot, d_slot) = cdp_slots(owner);
    writer(&ZUSD_VAULT_ADDRESS, c_slot, collateral_be);
    writer(&ZUSD_VAULT_ADDRESS, d_slot, debt_be);
}

/// Encode a u128 wei amount as 32 BE bytes (helper for tests / seeds).
pub fn encode_u128_be(value: u128) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[16..].copy_from_slice(&value.to_be_bytes());
    out
}

/// Re-export of the oracle helper so callers seeding both registries
/// only need one import path.
pub use crate::oracle_state::encode_price_e8;

/// Genesis assertion (called by node startup). Returns `Ok` iff a
/// non-empty bytecode lives at [`ZUSD_VAULT_ADDRESS`]. Production
/// loaders MUST call this against the same `code` accessor the
/// executor uses; failure indicates the chain spec did not include
/// `ZbxVaultRegistry.sol` at the canonical address and the 0x0F
/// precompile would silently report "non-existent vault" for every
/// owner — a consensus-relevant misconfiguration.
///
/// `code_at(addr) -> bytecode` is supplied by the caller (typically a
/// thin closure over `state.get_code(addr)`).
pub fn assert_vault_deployed<F>(code_at: F) -> Result<(), VaultGenesisError>
where
    F: FnOnce(&[u8; 20]) -> Vec<u8>,
{
    let code = code_at(&ZUSD_VAULT_ADDRESS);
    if code.is_empty() {
        return Err(VaultGenesisError::NotDeployed);
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub enum VaultGenesisError {
    /// No bytecode at [`ZUSD_VAULT_ADDRESS`] — `ZbxVaultRegistry.sol`
    /// is missing from genesis.
    NotDeployed,
}

impl core::fmt::Display for VaultGenesisError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotDeployed => write!(
                f,
                "ZbxVaultRegistry.sol must be deployed at 0x..5455 at genesis"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oracle_state::{encode_timestamp, write_feed};
    use std::collections::HashMap;

    /// Test reader. Implements `VaultStateReader` directly so tests can
    /// inject `current_timestamp` to exercise the freshness gate.
    #[derive(Default)]
    struct InMemReader {
        slots: HashMap<([u8; 20], [u8; 32]), [u8; 32]>,
        now: u64,
    }
    impl VaultStateReader for InMemReader {
        fn read_slot(&self, addr: &[u8; 20], slot: &[u8; 32]) -> [u8; 32] {
            self.slots.get(&(*addr, *slot)).copied().unwrap_or([0u8; 32])
        }
        fn current_timestamp(&self) -> u64 {
            self.now
        }
    }

    fn seed_zbx_price(r: &mut InMemReader, price_e8: u128, ts: u64) {
        let mut sym = [0u8; 32];
        sym.copy_from_slice(keccak256(ZBX_USD_FEED_SYMBOL).as_bytes());
        write_feed(
            &sym,
            encode_price_e8(price_e8),
            encode_timestamp(ts),
            |a, slot, v| {
                r.slots.insert((*a, slot), v);
            },
        );
    }

    fn seed_cdp(r: &mut InMemReader, owner: &[u8; 20], collateral: u128, debt: u128) {
        write_cdp(
            owner,
            encode_u128_be(collateral),
            encode_u128_be(debt),
            |a, slot, v| {
                r.slots.insert((*a, slot), v);
            },
        );
    }

    fn pad_addr(owner: &[u8; 20]) -> [u8; 32] {
        let mut p = [0u8; 32];
        p[12..].copy_from_slice(owner);
        p
    }

    #[test]
    fn input_must_be_32_bytes() {
        let r = InMemReader::default();
        assert!(matches!(
            do_zusd_vault_read(&[0u8; 31], 100_000, &r).unwrap_err(),
            VaultPrecompileError::BadInputLength { got: 31 }
        ));
        assert!(matches!(
            do_zusd_vault_read(&[0u8; 33], 100_000, &r).unwrap_err(),
            VaultPrecompileError::BadInputLength { got: 33 }
        ));
    }

    #[test]
    fn out_of_gas_below_total() {
        let r = InMemReader::default();
        let owner = [0x11u8; 20];
        let err = do_zusd_vault_read(&pad_addr(&owner), TOTAL_GAS - 1, &r).unwrap_err();
        assert_eq!(err, VaultPrecompileError::OutOfGas);
    }

    #[test]
    fn empty_vault_returns_128_zeros() {
        let r = InMemReader::default();
        let owner = [0x22u8; 20];
        let (out, gas) = do_zusd_vault_read(&pad_addr(&owner), 100_000, &r).unwrap();
        assert_eq!(gas, TOTAL_GAS);
        assert_eq!(out, vec![0u8; 128]);
    }

    #[test]
    fn funded_vault_healthy_ratio() {
        // 100 ZBX collateral at $2.00 (price_e8 = 2_00_000_000) →
        // collateral_value = $200. Debt = 50 ZUSD → c_ratio = 400% = 40_000 bps.
        // liq_price_e18 = debt * 1e18 / collateral = 50e18 * 1e18 / 100e18 = 0.5e18.
        let mut r = InMemReader::default();
        seed_zbx_price(&mut r, 2_00_000_000, 1_700_000_000);
        let owner = [0x33u8; 20];
        seed_cdp(&mut r, &owner, 100u128 * 10u128.pow(18), 50u128 * 10u128.pow(18));

        let (out, _) = do_zusd_vault_read(&pad_addr(&owner), 100_000, &r).unwrap();
        assert_eq!(out.len(), 128);
        assert_eq!(
            U256::from_big_endian(&out[0..32]),
            U256::from(100u128 * 10u128.pow(18))
        );
        assert_eq!(
            U256::from_big_endian(&out[32..64]),
            U256::from(50u128 * 10u128.pow(18))
        );
        assert_eq!(U256::from_big_endian(&out[64..96]), U256::from(40_000u64));
        assert_eq!(
            U256::from_big_endian(&out[96..128]),
            U256::from(500_000_000_000_000_000u128)
        );
    }

    #[test]
    fn undercollateralized_ratio_below_150pct() {
        // 100 ZBX at $1.00 = $100 collateral; debt = $80 → c_ratio = 125% = 12_500 bps.
        let mut r = InMemReader::default();
        seed_zbx_price(&mut r, 1_00_000_000, 1_700_000_000);
        let owner = [0x44u8; 20];
        seed_cdp(&mut r, &owner, 100u128 * 10u128.pow(18), 80u128 * 10u128.pow(18));

        let (out, _) = do_zusd_vault_read(&pad_addr(&owner), 100_000, &r).unwrap();
        let c_ratio = U256::from_big_endian(&out[64..96]);
        assert_eq!(c_ratio, U256::from(12_500u64));
        assert!(c_ratio < U256::from(15_000u64), "must report under 150 % CR");
    }

    #[test]
    fn vault_address_is_consensus_critical() {
        assert_eq!(ZUSD_VAULT_ADDRESS[18], 0x54);
        assert_eq!(ZUSD_VAULT_ADDRESS[19], 0x55);
        assert!(ZUSD_VAULT_ADDRESS[..18].iter().all(|&b| b == 0));
    }

    #[test]
    fn cdp_slots_are_distinct_and_consecutive() {
        let owner = [0x55u8; 20];
        let (c, d) = cdp_slots(&owner);
        assert_ne!(c, d);
        let mut expected_d = c;
        add_one_be(&mut expected_d);
        assert_eq!(d, expected_d);
    }

    #[test]
    fn cdps_map_slot_is_zero() {
        // Pinned to slot 0 — `ZbxVaultRegistry.sol::CDPS_MAP_SLOT()`
        // returns the same value via assembly. Adding state above
        // `cdps` in the registry (= bumping this slot) is a consensus
        // break; this test catches accidental drift.
        assert_eq!(CDPS_MAP_SLOT, [0u8; 32]);
    }

    #[test]
    fn debt_zero_yields_zero_derived_fields() {
        // Funded collateral, no debt → c_ratio + liq_price both 0 (no
        // div-by-zero panic). Raw collateral still reported; debt = 0.
        let mut r = InMemReader::default();
        seed_zbx_price(&mut r, 2_00_000_000, 1_700_000_000);
        let owner = [0x77u8; 20];
        seed_cdp(&mut r, &owner, 100u128 * 10u128.pow(18), 0);
        let (out, _) = do_zusd_vault_read(&pad_addr(&owner), 100_000, &r).unwrap();
        assert_eq!(
            U256::from_big_endian(&out[0..32]),
            U256::from(100u128 * 10u128.pow(18))
        );
        assert_eq!(U256::from_big_endian(&out[32..64]), U256::zero());
        assert_eq!(U256::from_big_endian(&out[64..96]), U256::zero());
        assert_eq!(U256::from_big_endian(&out[96..128]), U256::zero());
    }

    #[test]
    fn arithmetic_saturates_on_overflow() {
        // Pathological vault: collateral = 2^255, price_e8 ≈ 2^60.
        // Intermediate `collateral * price_e18` overflows U256, so the
        // saturating-fallback branch must fire and return U256::MAX
        // for c_ratio (NOT panic, NOT silently wrap to 0).
        let mut r = InMemReader::default();
        seed_zbx_price(&mut r, 1u128 << 60, 1);
        let owner = [0x88u8; 20];
        let mut huge = [0u8; 32];
        huge[0] = 0x80;
        write_cdp(&owner, huge, encode_u128_be(1u128 << 60), |a, slot, v| {
            r.slots.insert((*a, slot), v);
        });
        let (out, _) = do_zusd_vault_read(&pad_addr(&owner), 100_000, &r).unwrap();
        assert_eq!(out[64..96], [0xFFu8; 32]);
    }

    #[test]
    fn zero_oracle_price_yields_zero_derived_fields() {
        let mut r = InMemReader::default();
        let owner = [0x66u8; 20];
        seed_cdp(&mut r, &owner, 100u128 * 10u128.pow(18), 50u128 * 10u128.pow(18));
        let (out, _) = do_zusd_vault_read(&pad_addr(&owner), 100_000, &r).unwrap();
        assert_eq!(U256::from_big_endian(&out[64..96]), U256::zero());
        assert_eq!(U256::from_big_endian(&out[96..128]), U256::zero());
        assert_eq!(
            U256::from_big_endian(&out[0..32]),
            U256::from(100u128 * 10u128.pow(18))
        );
    }

    #[test]
    fn pass15_freshness_gate_blocks_stale_oracle() {
        // current_timestamp is set to 1h+1s past oracle ts → derived
        // fields zeroed. Raw collateral / debt still flow through.
        let mut r = InMemReader::default();
        let oracle_ts = 1_700_000_000u64;
        seed_zbx_price(&mut r, 2_00_000_000, oracle_ts);
        let owner = [0x99u8; 20];
        seed_cdp(&mut r, &owner, 100u128 * 10u128.pow(18), 50u128 * 10u128.pow(18));
        r.now = oracle_ts + MAX_ORACLE_STALENESS + 1;

        let (out, _) = do_zusd_vault_read(&pad_addr(&owner), 100_000, &r).unwrap();
        assert_eq!(U256::from_big_endian(&out[64..96]), U256::zero(),
            "stale oracle must zero c_ratio");
        assert_eq!(U256::from_big_endian(&out[96..128]), U256::zero(),
            "stale oracle must zero liq_price");
        // Raw fields still pass through.
        assert_eq!(
            U256::from_big_endian(&out[0..32]),
            U256::from(100u128 * 10u128.pow(18))
        );

        // Walk back inside the freshness window → derived fields restored.
        r.now = oracle_ts + MAX_ORACLE_STALENESS;
        let (out2, _) = do_zusd_vault_read(&pad_addr(&owner), 100_000, &r).unwrap();
        assert_eq!(U256::from_big_endian(&out2[64..96]), U256::from(40_000u64));
    }

    #[test]
    fn pass15_freshness_skipped_when_host_reports_zero_now() {
        // Default `current_timestamp = 0` (legacy path) ⇒ stale oracle
        // is NOT gated. This preserves byte-for-byte equivalence with
        // pre-Pass-15 callers and with tests that don't bother with
        // freshness.
        let mut r = InMemReader::default();
        seed_zbx_price(&mut r, 2_00_000_000, 1_700_000_000);
        let owner = [0xAAu8; 20];
        seed_cdp(&mut r, &owner, 100u128 * 10u128.pow(18), 50u128 * 10u128.pow(18));
        r.now = 0;
        let (out, _) = do_zusd_vault_read(&pad_addr(&owner), 100_000, &r).unwrap();
        assert_eq!(U256::from_big_endian(&out[64..96]), U256::from(40_000u64));
    }

    #[test]
    fn assert_vault_deployed_passes_when_code_present() {
        let res = assert_vault_deployed(|addr| {
            assert_eq!(addr, &ZUSD_VAULT_ADDRESS);
            vec![0x60, 0x80, 0x60, 0x40, 0x52] // bytecode prelude
        });
        assert!(res.is_ok());
    }

    #[test]
    fn assert_vault_deployed_fails_when_address_empty() {
        let res = assert_vault_deployed(|_| Vec::new());
        assert_eq!(res.unwrap_err(), VaultGenesisError::NotDeployed);
    }
}
