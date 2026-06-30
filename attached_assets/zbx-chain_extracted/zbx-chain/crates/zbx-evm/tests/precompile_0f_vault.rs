//! Task #7 (Precompile 0x0F — ZUSD vault state direct-read): EVM-side tests.
//!
//! Verifies (a) `is_precompile` extends to 0x0F, (b) the wrapper
//! `zbx_evm::precompiles::do_zusd_vault` produces results byte-identical
//! to the shared body in `zbx_crypto::vault_state::do_zusd_vault_read`
//! (cross-VM equivalence is therefore guaranteed by construction with
//! the ZVM wrapper, which calls the same body).

use std::collections::HashMap;
use zbx_crypto::keccak::keccak256;
use zbx_crypto::oracle_state::{encode_price_e8, encode_timestamp, write_feed};
use zbx_crypto::vault_state::{
    do_zusd_vault_read as shared_vault, encode_u128_be, write_cdp, VaultStateReader, TOTAL_GAS,
    ZBX_USD_FEED_SYMBOL,
};
use zbx_evm::error::EvmError;
use zbx_evm::precompiles::{do_zusd_vault, is_precompile};
use zbx_types::address::Address;

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
    write_cdp(owner, encode_u128_be(collateral), encode_u128_be(debt), |a, slot, v| {
        r.0.insert((*a, slot), v);
    });
}

#[test]
fn evm_is_precompile_includes_0f() {
    let mut a = [0u8; 20];
    a[19] = 0x0F;
    assert!(is_precompile(&Address(a)));
    a[19] = 0x10;
    assert!(!is_precompile(&Address(a)));
}

#[test]
fn evm_empty_vault_returns_128_zeros() {
    let r = InMemReader::default();
    let owner = [0x11u8; 20];
    let (out, gas) = do_zusd_vault(&pad(&owner), 100_000, &r).unwrap();
    assert_eq!(gas, TOTAL_GAS);
    assert_eq!(out, vec![0u8; 128]);
}

#[test]
fn evm_oog_maps_to_evm_oog() {
    let r = InMemReader::default();
    let err = do_zusd_vault(&pad(&[0u8; 20]), TOTAL_GAS - 1, &r).unwrap_err();
    assert!(matches!(err, EvmError::OutOfGas));
}

#[test]
fn evm_bad_input_length() {
    let r = InMemReader::default();
    let err = do_zusd_vault(&[0u8; 33], 100_000, &r).unwrap_err();
    assert!(matches!(err, EvmError::Precompile(_)));
}

#[test]
fn evm_byte_identical_to_shared_body() {
    let mut r = InMemReader::default();
    seed_zbx_price(&mut r, 2_00_000_000, 1_700_000_000);
    let owners = [[0x21u8; 20], [0x22u8; 20], [0x23u8; 20]];
    seed_cdp(&mut r, &owners[0], 100u128 * 10u128.pow(18), 50u128 * 10u128.pow(18));
    seed_cdp(&mut r, &owners[1], 1_000u128 * 10u128.pow(18), 800u128 * 10u128.pow(18));
    // owners[2] left empty
    for o in &owners {
        let evm = do_zusd_vault(&pad(o), 100_000, &r).unwrap();
        let shared = shared_vault(&pad(o), 100_000, &r).unwrap();
        assert_eq!(evm.0, shared.0, "{:?}: EVM wrapper drift from shared body", o);
        assert_eq!(evm.1, shared.1, "{:?}: gas drift from shared body", o);
    }
}

#[test]
fn evm_undercollateralized_below_150pct() {
    // Collateral $100 (100 ZBX × $1), debt $80 → c_ratio = 125% = 12_500 bps.
    let mut r = InMemReader::default();
    seed_zbx_price(&mut r, 1_00_000_000, 1_700_000_000);
    let owner = [0x44u8; 20];
    seed_cdp(&mut r, &owner, 100u128 * 10u128.pow(18), 80u128 * 10u128.pow(18));
    let (out, _) = do_zusd_vault(&pad(&owner), 100_000, &r).unwrap();
    let mut expected = [0u8; 32];
    expected[30..].copy_from_slice(&12_500u16.to_be_bytes());
    assert_eq!(&out[64..96], &expected[..]);
    assert!(out[64..94].iter().all(|&b| b == 0));
}
