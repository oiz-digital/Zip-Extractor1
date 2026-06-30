//! Task #5 (Precompile 0x0C — Price oracle read): ZVM-side tests.
//!
//! The shared body lives in `zbx_crypto::oracle_state`; here we exercise
//! the ZVM wrapper `zbx_zvm::precompiles::price_oracle_with` which
//! routes ZVM `OutOfGas` / `InvalidInput` errors and is the function
//! the interpreter actually invokes.

use std::collections::HashMap;
use zbx_crypto::oracle_state::{
    do_price_oracle, encode_price_e8, encode_timestamp, write_feed, OracleStateReader,
    ORACLE_REGISTRY_ADDRESS, TOTAL_GAS,
};
use zbx_crypto::keccak::keccak256;
use zbx_zvm::error::ZvmError;
use zbx_zvm::precompiles::price_oracle_with;

#[derive(Default)]
struct InMemReader(HashMap<([u8; 20], [u8; 32]), [u8; 32]>);

impl OracleStateReader for InMemReader {
    fn read_slot(&self, addr: &[u8; 20], slot: &[u8; 32]) -> [u8; 32] {
        self.0.get(&(*addr, *slot)).copied().unwrap_or([0u8; 32])
    }
}

fn sym(name: &str) -> [u8; 32] {
    let mut h = [0u8; 32];
    h.copy_from_slice(keccak256(name.as_bytes()).as_bytes());
    h
}

fn seed(r: &mut InMemReader, name: &str, p: u128, t: u64) {
    write_feed(&sym(name), encode_price_e8(p), encode_timestamp(t), |a, slot, v| {
        r.0.insert((*a, slot), v);
    });
}

#[test]
fn zvm_known_feed_returns_real_layout() {
    let mut r = InMemReader::default();
    seed(&mut r, "BTC/USD", 67_500_00_000_000, 1_700_000_000);
    let (out, gas) = price_oracle_with(&sym("BTC/USD"), 100_000, &r).unwrap();
    assert_eq!(gas, TOTAL_GAS, "Task #5 gas schedule = 1200");
    assert_eq!(out.len(), 64);
    assert_eq!(&out[0..32], &encode_price_e8(67_500_00_000_000)[..]);
    assert_eq!(&out[32..64], &encode_timestamp(1_700_000_000)[..]);
}

#[test]
fn zvm_unknown_feed_returns_zeros_no_revert() {
    let r = InMemReader::default();
    let (out, gas) = price_oracle_with(&sym("NOT/LISTED"), 100_000, &r).unwrap();
    assert_eq!(gas, TOTAL_GAS);
    assert_eq!(out, vec![0u8; 64], "unknown feed must return 64 zero bytes");
}

#[test]
fn zvm_oog_below_total() {
    let r = InMemReader::default();
    let err = price_oracle_with(&sym("BTC/USD"), TOTAL_GAS - 1, &r).unwrap_err();
    assert!(matches!(err, ZvmError::OutOfGas));
}

#[test]
fn zvm_bad_input_length() {
    let r = InMemReader::default();
    let err = price_oracle_with(&[0u8; 31], 100_000, &r).unwrap_err();
    assert!(matches!(err, ZvmError::InvalidInput(_)));
}

#[test]
fn zvm_uses_well_known_registry_address() {
    // Ensure the precompile actually reads from ORACLE_REGISTRY_ADDRESS,
    // not some other address. We seed via the helper (which writes there)
    // and check that a reader which only serves that address still works.
    struct OnlyRegistryReader(HashMap<[u8; 32], [u8; 32]>);
    impl OracleStateReader for OnlyRegistryReader {
        fn read_slot(&self, addr: &[u8; 20], slot: &[u8; 32]) -> [u8; 32] {
            assert_eq!(
                addr, &ORACLE_REGISTRY_ADDRESS,
                "precompile must read from registry address only",
            );
            self.0.get(slot).copied().unwrap_or([0u8; 32])
        }
    }
    let mut by_slot = HashMap::new();
    write_feed(
        &sym("ZBX/USD"),
        encode_price_e8(2_50_000_000),
        encode_timestamp(1_700_000_999),
        |_a, slot, v| {
            by_slot.insert(slot, v);
        },
    );
    let r = OnlyRegistryReader(by_slot);
    let (out, _) = price_oracle_with(&sym("ZBX/USD"), 100_000, &r).unwrap();
    assert_eq!(&out[0..32], &encode_price_e8(2_50_000_000)[..]);
}

#[test]
fn zvm_byte_identical_to_evm() {
    // Cross-VM consensus equivalence: feed identical input + reader
    // through both wrappers; outputs and gas must match exactly.
    let mut zvm_r = InMemReader::default();
    let mut evm_r = InMemReader::default();
    for (name, p, t) in [
        ("BTC/USD",  67_500_00_000_000u128, 1_700_000_000u64),
        ("ETH/USD",  3_500_00_000_000,      1_700_000_001),
        ("ZBX/USD",  2_50_000_000,          1_700_000_002),
        ("MISSING",  0,                     0), // will be overwritten with zeros
    ] {
        if p != 0 || t != 0 {
            seed(&mut zvm_r, name, p, t);
            seed(&mut evm_r, name, p, t);
        }
    }

    for name in ["BTC/USD", "ETH/USD", "ZBX/USD", "MISSING", "DOES/NOT/EXIST"] {
        let s = sym(name);
        let z = price_oracle_with(&s, 100_000, &zvm_r).unwrap();
        // EVM path goes through the same shared body; verify identical (out, gas).
        let e = do_price_oracle(&s, 100_000, &evm_r).unwrap();
        assert_eq!(z.0, e.0, "{name} output mismatch ZVM vs shared");
        assert_eq!(z.1, e.1, "{name} gas mismatch ZVM vs shared");
    }
}
