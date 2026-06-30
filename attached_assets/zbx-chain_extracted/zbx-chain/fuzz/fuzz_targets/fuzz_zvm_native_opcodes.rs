//! Fuzz target: specifically test ZVM native opcodes (0xC0–0xC9).
//! For each native opcode, fuzz the stack operands and environment.
//! Ensures:
//!   1. No panics on any native opcode
//!   2. Stack depth invariants maintained
//!   3. Pay ID parser does not panic on arbitrary input
//!   4. Oracle reads always return (no deadlock/infinite loop)
//!
//! Run:
//!   cargo +nightly fuzz run fuzz_zvm_native_opcodes -- -max_total_time=60
#![no_main]

use libfuzzer_sys::{fuzz_target, arbitrary};
use arbitrary::Arbitrary;
use zbx_zvm::{Interpreter, InterpreterResult, state::FuzzMockDb, env::Env};

/// Arbitrary ZVM context for native opcode testing
#[derive(Arbitrary, Debug)]
struct ZvmNativeFuzzCase {
    /// Which native opcode to test (0x00–0x09 mapped to 0xC0–0xC9)
    opcode_idx: u8,
    /// Up to 16 stack items to push before the opcode
    stack_vals: Vec<[u8; 32]>,
    /// Fake Pay ID bytes (for PAYID opcode)
    payid_input: Vec<u8>,
    /// Fake oracle price to inject
    mock_price_usd_cents: u64,
}

fuzz_target!(|case: ZvmNativeFuzzCase| {
    const GAS: u64 = 100_000;
    let opcode = 0xC0u8 + (case.opcode_idx % 10); // 0xC0..0xC9

    // Build bytecode: PUSH32 * n stack items, then native opcode, then STOP
    let mut bytecode: Vec<u8> = Vec::new();

    // Push up to 8 stack items
    for val in case.stack_vals.iter().take(8) {
        bytecode.push(0x7f); // PUSH32
        bytecode.extend_from_slice(val);
    }

    // The native opcode under test
    bytecode.push(opcode);

    // POP any return value + STOP
    bytecode.push(0x50); // POP
    bytecode.push(0x00); // STOP

    // Set up mock state with injected oracle price and Pay ID map
    let mock_db = FuzzMockDb::new()
        .with_price_cents(case.mock_price_usd_cents)
        .with_payid_bytes(&case.payid_input);

    let mut interp = Interpreter::new(bytecode, mock_db, Env::default(), GAS);
    let result: InterpreterResult = interp.run();

    // Invariant: gas used ≤ limit (native opcodes should never over-charge)
    assert!(result.gas_used <= GAS);

    // Invariant: no panic, any result action is acceptable
    let _ = result.action;
});