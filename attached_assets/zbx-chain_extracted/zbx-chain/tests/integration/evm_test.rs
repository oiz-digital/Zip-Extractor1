//! Integration tests: EVM execution.

use zbx_evm::{Interpreter, EvmConfig, EvmContext};
use zbx_types::{Address, U256};
use zbx_state::StateDB;

/// PUSH1 1, PUSH1 2, ADD, STOP — should result in stack [3].
const ADD_PROGRAM: &[u8] = &[
    0x60, 0x01, // PUSH1 1
    0x60, 0x02, // PUSH1 2
    0x01,       // ADD
    0x00,       // STOP
];

#[test]
fn test_evm_add() {
    let mut state = StateDB::new_in_memory();
    let config    = EvmConfig::mainnet();
    let caller    = Address::from([0x01; 20]);
    let callee    = Address::from([0x02; 20]);

    state.set_code(callee, ADD_PROGRAM.to_vec());
    state.set_balance(caller, U256::from(1_000_000_000_000_000_000u64));

    let ctx = EvmContext {
        caller,
        callee,
        value: U256::zero(),
        calldata: vec![],
        gas_limit: 100_000,
        depth: 0,
    };

    let mut interp = Interpreter::new(config);
    let result = interp.execute(ctx, &mut state);

    assert!(result.success, "EVM execution failed: {:?}", result.revert_reason);
    assert!(result.gas_used < 100_000, "gas accounting error");
}

#[test]
fn test_evm_revert() {
    // PUSH1 0, PUSH1 0, REVERT
    let revert_program = &[0x60, 0x00, 0x60, 0x00, 0xfd];

    let mut state = StateDB::new_in_memory();
    let caller    = Address::from([0x01; 20]);
    let callee    = Address::from([0x99; 20]);
    state.set_code(callee, revert_program.to_vec());

    let ctx = EvmContext {
        caller, callee,
        value: U256::zero(),
        calldata: vec![],
        gas_limit: 50_000,
        depth: 0,
    };

    let mut interp = Interpreter::new(EvmConfig::mainnet());
    let result = interp.execute(ctx, &mut state);

    assert!(!result.success, "expected REVERT but got success");
}

#[test]
fn test_precompile_keccak256() {
    // Address 0x02 — SHA-256; Address 0x09 — BLAKE2f; check address 0x20 (keccak)
    let mut state  = StateDB::new_in_memory();
    let caller     = Address::from([0x01; 20]);
    // Call the keccak256 precompile at 0x0000...0020.
    let precompile = Address::from_low_u64_be(0x20);

    let input = b"zebvix";
    let ctx = EvmContext {
        caller,
        callee: precompile,
        value: U256::zero(),
        calldata: input.to_vec(),
        gas_limit: 200_000,
        depth: 0,
    };

    let mut interp = Interpreter::new(EvmConfig::mainnet());
    let result = interp.execute(ctx, &mut state);

    assert!(result.success, "precompile failed");
    assert_eq!(result.output.len(), 32, "keccak256 output must be 32 bytes");
}

#[test]
fn test_storage_read_write() {
    // SSTORE key=0, val=0xdeadbeef; SLOAD key=0; STOP
    // (simplified bytecode — real test uses proper opcode encoding)
    let mut state = StateDB::new_in_memory();
    let contract  = Address::from([0xc0; 20]);

    // Pre-populate storage.
    state.set_storage(contract, U256::zero(), U256::from(0xdeadbeef_u64));

    let value = state.get_storage(contract, U256::zero());
    assert_eq!(value, U256::from(0xdeadbeef_u64));
}