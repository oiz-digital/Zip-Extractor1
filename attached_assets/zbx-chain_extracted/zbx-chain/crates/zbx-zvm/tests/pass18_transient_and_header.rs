//! SEC-2026-05-09 Pass-18 — `MockZvmHost` now provides real EIP-1153
//! transient-storage scratchpad and header-derived fields (coinbase,
//! prevrandao, gas_price, blob_hash, block_gas_limit). Pre-Pass-18 the
//! defaults returned zeros, which silently bricked every Cancun-era
//! reentrancy guard, every `block.coinbase`/`block.prevrandao` branch,
//! and every EIP-4844 blob-aware contract.

use zbx_zvm::host::{MockZvmHost, ZvmHost};

#[test]
fn transient_load_default_is_zero() {
    let h = MockZvmHost::new();
    let addr = [0xAAu8; 20];
    let key = [0x01u8; 32];
    assert_eq!(h.transient_load(&addr, &key), [0u8; 32]);
}

#[test]
fn transient_store_then_load_roundtrips() {
    let mut h = MockZvmHost::new();
    let addr = [0xBBu8; 20];
    let key = [0x42u8; 32];
    let value = [0xFFu8; 32];
    h.transient_store(&addr, key, value);
    assert_eq!(h.transient_load(&addr, &key), value);
}

#[test]
fn transient_storage_isolated_per_address() {
    // EIP-1153 scratchpad is keyed on (address, slot) — writes to one
    // contract must not leak into another, just like persistent storage.
    let mut h = MockZvmHost::new();
    let alice = [0x01u8; 20];
    let bob   = [0x02u8; 20];
    let key   = [0x99u8; 32];
    let v     = [0x77u8; 32];
    h.transient_store(&alice, key, v);
    assert_eq!(h.transient_load(&alice, &key), v);
    assert_eq!(h.transient_load(&bob, &key), [0u8; 32]);
}

#[test]
fn clear_transient_wipes_scratchpad_for_new_tx() {
    // Production host calls clear_transient() at the end of every
    // transaction — the scratchpad must NOT survive across txs.
    let mut h = MockZvmHost::new();
    let addr = [0xCCu8; 20];
    let key = [0x55u8; 32];
    let v = [0xAAu8; 32];
    h.transient_store(&addr, key, v);
    assert_eq!(h.transient_load(&addr, &key), v);

    h.clear_transient();
    assert_eq!(h.transient_load(&addr, &key), [0u8; 32]);
}

#[test]
fn transient_overwrite_replaces_previous_value() {
    let mut h = MockZvmHost::new();
    let addr = [0xDDu8; 20];
    let key = [0x01u8; 32];
    h.transient_store(&addr, key, [0x11u8; 32]);
    h.transient_store(&addr, key, [0x22u8; 32]);
    assert_eq!(h.transient_load(&addr, &key), [0x22u8; 32]);
}

#[test]
fn header_fields_propagate_from_block() {
    let mut h = MockZvmHost::new();
    let cb = [0xABu8; 20];
    let prand = [0xCDu8; 32];
    let blob1 = [0x11u8; 32];
    let blob2 = [0x22u8; 32];
    h.coinbase = cb;
    h.block_gas_limit = 60_000_000;
    h.prevrandao = prand;
    h.gas_price = 7_000_000_000u128;
    h.blob_hashes = vec![blob1, blob2];

    assert_eq!(h.coinbase(), cb);
    assert_eq!(h.block_gas_limit(), 60_000_000);
    assert_eq!(h.prevrandao(), prand);
    assert_eq!(h.gas_price(), 7_000_000_000u128);
    assert_eq!(h.blob_hash(0), blob1);
    assert_eq!(h.blob_hash(1), blob2);
}

#[test]
fn blob_hash_out_of_range_returns_zero() {
    // EIP-4844: BLOBHASH of an out-of-range index returns the zero hash.
    let mut h = MockZvmHost::new();
    h.blob_hashes = vec![[0xAAu8; 32]];
    assert_eq!(h.blob_hash(0), [0xAAu8; 32]);
    assert_eq!(h.blob_hash(1), [0u8; 32]);
    assert_eq!(h.blob_hash(999), [0u8; 32]);
}

#[test]
fn defaults_match_pre_pass18_zero_behavior() {
    // Backward-compat: a freshly-constructed MockZvmHost still returns
    // zero defaults (matching pre-Pass-18 semantics) — Pass-18 only
    // enables overrides, it does not change defaults.
    let h = MockZvmHost::new();
    assert_eq!(h.coinbase(), [0u8; 20]);
    assert_eq!(h.block_gas_limit(), 30_000_000);
    assert_eq!(h.prevrandao(), [0u8; 32]);
    assert_eq!(h.gas_price(), 0);
    assert_eq!(h.blob_hash(0), [0u8; 32]);
}
