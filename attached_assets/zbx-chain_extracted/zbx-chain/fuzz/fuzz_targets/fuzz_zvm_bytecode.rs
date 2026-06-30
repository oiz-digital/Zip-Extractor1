//! Fuzz target: feed arbitrary bytecode to ZVM and ensure:
//!   1. No panics (undefined behaviour)
//!   2. Execution terminates (no infinite loops beyond gas limit)
//!   3. Gas accounting stays consistent (used ≤ limit always)
//!   4. Memory safety (no OOB stack/memory access)
//!
//! Run:
//!   cargo +nightly fuzz run fuzz_zvm_bytecode -- -max_total_time=60
#![no_main]

use libfuzzer_sys::fuzz_target;
use zbx_zvm::{
    Interpreter, InterpreterAction, InterpreterResult,
    gas::{GasLimit, GasUsed},
    state::NoopDb,
    env::{Env, TxEnv},
};

fuzz_target!(|data: &[u8]| {
    // Always enforce a hard gas limit so execution terminates
    const MAX_GAS: u64 = 1_000_000;

    // Skip empty input — no interesting coverage
    if data.is_empty() { return; }

    let env = Env {
        tx: TxEnv {
            caller:   [0u8; 20].into(),
            gas_limit: MAX_GAS,
            ..Default::default()
        },
        ..Default::default()
    };

    let mut interp = Interpreter::new(
        data.to_vec(),       // arbitrary bytecode
        NoopDb,              // stateless stub
        env,
        MAX_GAS,
    );

    // Execute — must never panic regardless of bytecode content
    let result: InterpreterResult = interp.run();

    // Invariant 1: gas used must never exceed gas limit
    assert!(
        result.gas_used <= MAX_GAS,
        "Gas accounting error: used {} > limit {}",
        result.gas_used,
        MAX_GAS
    );

    // Invariant 2: result must be one of the defined variants (no unknown states)
    match result.action {
        InterpreterAction::Return { .. }  => {}
        InterpreterAction::Revert { .. }  => {}
        InterpreterAction::Stop           => {}
        InterpreterAction::OutOfGas       => {}
        InterpreterAction::InvalidOpcode  => {}
        InterpreterAction::StackOverflow  => {}
        InterpreterAction::StackUnderflow => {}
    }
});