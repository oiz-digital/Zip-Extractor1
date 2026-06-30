//! Task #7 (Precompile 0x0F — ZUSD vault state direct-read): ZVM-side tests.
//!
//! Two tiers of coverage:
//!
//! 1. Unit-level: an in-memory `VaultStateReader` exercises the
//!    wrapper `zbx_zvm::precompiles::zusd_vault_with` (the function the
//!    interpreter actually calls) for all empty/funded/OOG/length cases.
//! 2. Executor-level: a real `MockZvmHost` is populated via
//!    `host.storage_store(...)` exactly as a Solidity SSTORE would, and
//!    a thin adapter (byte-identical to the one in
//!    `zbx-zvm/src/interpreter.rs`) routes precompile reads through
//!    `ZvmHost::storage_load`. This proves the precompile is wired
//!    against the actual on-chain storage layout used by
//!    `ZbxVaultRegistry.sol`.

use std::collections::HashMap;
use zbx_crypto::keccak::keccak256;
use zbx_crypto::oracle_state::{encode_price_e8, encode_timestamp, write_feed};
use zbx_crypto::vault_state::{
    encode_u128_be, write_cdp, VaultStateReader, TOTAL_GAS, ZBX_USD_FEED_SYMBOL,
    ZUSD_VAULT_ADDRESS,
};
use zbx_zvm::error::ZvmError;
use zbx_zvm::host::{MockZvmHost, ZvmHost};
use zbx_zvm::precompiles::zusd_vault_with;

#[derive(Default)]
struct InMemReader(HashMap<([u8; 20], [u8; 32]), [u8; 32]>);
impl VaultStateReader for InMemReader {
    fn read_slot(&self, addr: &[u8; 20], slot: &[u8; 32]) -> [u8; 32] {
        self.0.get(&(*addr, *slot)).copied().unwrap_or([0u8; 32])
    }
}

fn pad(owner: &[u8; 20]) -> [u8; 32] {
    let mut p = [0u8; 32];
    p[12..].copy_from_slice(owner);
    p
}

fn seed_zbx_price(r: &mut InMemReader, p: u128, t: u64) {
    let mut sym = [0u8; 32];
    sym.copy_from_slice(keccak256(ZBX_USD_FEED_SYMBOL).as_bytes());
    write_feed(&sym, encode_price_e8(p), encode_timestamp(t), |a, slot, v| {
        r.0.insert((*a, slot), v);
    });
}

fn seed_cdp(r: &mut InMemReader, owner: &[u8; 20], collateral: u128, debt: u128) {
    write_cdp(
        owner,
        encode_u128_be(collateral),
        encode_u128_be(debt),
        |a, slot, v| {
            r.0.insert((*a, slot), v);
        },
    );
}

#[test]
fn zvm_empty_vault_returns_128_zeros() {
    let r = InMemReader::default();
    let owner = [0xAAu8; 20];
    let (out, gas) = zusd_vault_with(&pad(&owner), 100_000, &r).unwrap();
    assert_eq!(gas, TOTAL_GAS);
    assert_eq!(out, vec![0u8; 128]);
}

#[test]
fn zvm_funded_vault_layout() {
    let mut r = InMemReader::default();
    seed_zbx_price(&mut r, 2_00_000_000, 1_700_000_000);
    let owner = [0xBBu8; 20];
    seed_cdp(&mut r, &owner, 100u128 * 10u128.pow(18), 50u128 * 10u128.pow(18));
    let (out, gas) = zusd_vault_with(&pad(&owner), 100_000, &r).unwrap();
    assert_eq!(gas, TOTAL_GAS);
    assert_eq!(out.len(), 128);
}

#[test]
fn zvm_oog() {
    let r = InMemReader::default();
    let err = zusd_vault_with(&pad(&[0u8; 20]), TOTAL_GAS - 1, &r).unwrap_err();
    assert!(matches!(err, ZvmError::OutOfGas));
}

#[test]
fn zvm_bad_input_length() {
    let r = InMemReader::default();
    let err = zusd_vault_with(&[0u8; 31], 100_000, &r).unwrap_err();
    assert!(matches!(err, ZvmError::InvalidInput(_)));
}

#[test]
fn zvm_uses_canonical_vault_address() {
    struct PinningReader(HashMap<([u8; 20], [u8; 32]), [u8; 32]>);
    impl VaultStateReader for PinningReader {
        fn read_slot(&self, addr: &[u8; 20], slot: &[u8; 32]) -> [u8; 32] {
            assert!(
                addr == &ZUSD_VAULT_ADDRESS
                    || addr == &zbx_crypto::oracle_state::ORACLE_REGISTRY_ADDRESS,
                "precompile read from unexpected address {:?}",
                addr
            );
            self.0.get(&(*addr, *slot)).copied().unwrap_or([0u8; 32])
        }
    }
    let mut by_slot = HashMap::new();
    let owner = [0xCCu8; 20];
    write_cdp(
        &owner,
        encode_u128_be(10u128 * 10u128.pow(18)),
        encode_u128_be(5u128 * 10u128.pow(18)),
        |a, slot, v| {
            by_slot.insert((*a, slot), v);
        },
    );
    let mut sym = [0u8; 32];
    sym.copy_from_slice(keccak256(ZBX_USD_FEED_SYMBOL).as_bytes());
    write_feed(&sym, encode_price_e8(1_00_000_000), encode_timestamp(1), |a, slot, v| {
        by_slot.insert((*a, slot), v);
    });
    let r = PinningReader(by_slot);
    let (out, _) = zusd_vault_with(&pad(&owner), 100_000, &r).unwrap();
    use primitive_types::U256;
    assert_eq!(U256::from_big_endian(&out[64..96]), U256::from(20_000u64));
}

// ─────────────────────────────────────────────────────────────────────────────
// Executor-level integration: MockZvmHost-backed adapter mirroring the one
// inlined in `zbx-zvm/src/interpreter.rs`. Storage is populated via
// `host.storage_store(...)` exactly as a Solidity SSTORE on
// ZbxVaultRegistry.cdps[owner].collateral / .debt would do, then the
// precompile is called against the live host. Cross-checks the slot-0
// `cdps` mapping layout end-to-end.
// ─────────────────────────────────────────────────────────────────────────────

/// Adapter — byte-identical to the one in `interpreter.rs`. Bridges
/// `ZvmHost::storage_load` to `VaultStateReader::read_slot`.
struct HostBackedReader<'a, H: ZvmHost + ?Sized>(&'a H, u64);
impl<H: ZvmHost + ?Sized> VaultStateReader for HostBackedReader<'_, H> {
    fn read_slot(&self, addr: &[u8; 20], slot: &[u8; 32]) -> [u8; 32] {
        self.0.storage_load(addr, slot)
    }
    fn current_timestamp(&self) -> u64 {
        self.1
    }
}

fn store_cdp_via_host(host: &mut MockZvmHost, owner: &[u8; 20], collateral: u128, debt: u128) {
    let (c_slot, d_slot) = zbx_crypto::vault_state::cdp_slots(owner);
    host.storage_store(&ZUSD_VAULT_ADDRESS, c_slot, encode_u128_be(collateral));
    host.storage_store(&ZUSD_VAULT_ADDRESS, d_slot, encode_u128_be(debt));
}

fn store_oracle_via_host(host: &mut MockZvmHost, price_e8: u128, ts: u64) {
    let mut sym = [0u8; 32];
    sym.copy_from_slice(keccak256(ZBX_USD_FEED_SYMBOL).as_bytes());
    let (p_slot, t_slot) = zbx_crypto::oracle_state::slot_pair(&sym);
    host.storage_store(
        &zbx_crypto::oracle_state::ORACLE_REGISTRY_ADDRESS,
        p_slot,
        encode_price_e8(price_e8),
    );
    host.storage_store(
        &zbx_crypto::oracle_state::ORACLE_REGISTRY_ADDRESS,
        t_slot,
        encode_timestamp(ts),
    );
}

#[test]
fn executor_level_funded_vault_via_real_host_storage() {
    use primitive_types::U256;
    let mut host = MockZvmHost::new();
    let owner = [0xDDu8; 20];
    // 100 ZBX collateral, 50 ZUSD debt, $2.00 price → 400% CR.
    store_cdp_via_host(&mut host, &owner, 100u128 * 10u128.pow(18), 50u128 * 10u128.pow(18));
    store_oracle_via_host(&mut host, 2_00_000_000, 1_700_000_000);

    let reader = HostBackedReader(&host, 0);
    let (out, gas) = zusd_vault_with(&pad(&owner), 100_000, &reader).unwrap();
    assert_eq!(gas, TOTAL_GAS);
    assert_eq!(out.len(), 128);
    // Field-by-field assertions against on-host storage.
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
fn executor_level_empty_vault_via_real_host_storage() {
    let host = MockZvmHost::new();
    let owner = [0xEEu8; 20];
    let reader = HostBackedReader(&host, 0);
    let (out, _) = zusd_vault_with(&pad(&owner), 100_000, &reader).unwrap();
    assert_eq!(out, vec![0u8; 128]);
}

#[test]
fn executor_level_undercollateralized_via_real_host_storage() {
    use primitive_types::U256;
    let mut host = MockZvmHost::new();
    let owner = [0xEFu8; 20];
    // 100 ZBX at $1.00 collateral, 80 ZUSD debt → 125 % CR (12_500 bps, < 150 %).
    store_cdp_via_host(&mut host, &owner, 100u128 * 10u128.pow(18), 80u128 * 10u128.pow(18));
    store_oracle_via_host(&mut host, 1_00_000_000, 1_700_000_000);
    let reader = HostBackedReader(&host, 0);
    let (out, _) = zusd_vault_with(&pad(&owner), 100_000, &reader).unwrap();
    assert_eq!(U256::from_big_endian(&out[64..96]), U256::from(12_500u64));
}

#[test]
fn executor_level_pass15_freshness_gates_stale_oracle() {
    use primitive_types::U256;
    use zbx_crypto::vault_state::MAX_ORACLE_STALENESS;
    let mut host = MockZvmHost::new();
    let owner = [0xF0u8; 20];
    let oracle_ts = 1_700_000_000u64;
    store_cdp_via_host(&mut host, &owner, 100u128 * 10u128.pow(18), 50u128 * 10u128.pow(18));
    store_oracle_via_host(&mut host, 2_00_000_000, oracle_ts);

    // Stale: now = oracle_ts + MAX_STALENESS + 1 → derived fields zeroed.
    let reader = HostBackedReader(&host, oracle_ts + MAX_ORACLE_STALENESS + 1);
    let (out, _) = zusd_vault_with(&pad(&owner), 100_000, &reader).unwrap();
    assert_eq!(U256::from_big_endian(&out[64..96]), U256::zero());
    assert_eq!(U256::from_big_endian(&out[96..128]), U256::zero());

    // Fresh: now exactly at MAX_STALENESS → derived fields restored.
    let reader2 = HostBackedReader(&host, oracle_ts + MAX_ORACLE_STALENESS);
    let (out2, _) = zusd_vault_with(&pad(&owner), 100_000, &reader2).unwrap();
    assert_eq!(U256::from_big_endian(&out2[64..96]), U256::from(40_000u64));
}
