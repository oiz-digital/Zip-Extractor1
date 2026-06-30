//! Property-based tests for ZVM using proptest.
//! These run in CI (unlike cargo-fuzz which needs nightly and manual runs).
//!
//! Run: cargo test --test proptest_zvm -p zbx-zvm
use proptest::prelude::*;
use zbx_zvm::{Interpreter, state::NoopDb, env::Env};

const MAX_GAS: u64 = 1_000_000;

/// Strategy: generate bytecode as arbitrary PUSH1 + arithmetic sequences
fn push1_arith_strategy() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(
        prop_oneof![
            // PUSH1 <value>
            (0u8..=255u8).prop_map(|v| vec![0x60u8, v]),
            // Arithmetic opcodes (binary — need 2 items on stack)
            Just(vec![0x01u8]), // ADD
            Just(vec![0x02u8]), // MUL
            Just(vec![0x03u8]), // SUB
            // Comparison
            Just(vec![0x10u8]), // LT
            Just(vec![0x11u8]), // GT
            Just(vec![0x14u8]), // EQ
            Just(vec![0x15u8]), // ISZERO (unary)
            Just(vec![0x16u8]), // AND
            Just(vec![0x17u8]), // OR
            Just(vec![0x18u8]), // XOR
            Just(vec![0x19u8]), // NOT (unary)
            // Stack
            Just(vec![0x50u8]), // POP
        ],
        0..=20usize,
    )
    .prop_map(|segs| {
        let mut bytecode: Vec<u8> = segs.into_iter().flatten().collect();
        bytecode.push(0x00); // STOP
        bytecode
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(5_000))]

    /// Property: gas used is always ≤ gas limit, regardless of bytecode
    #[test]
    fn prop_gas_never_exceeds_limit(bytecode in push1_arith_strategy()) {
        let mut interp = Interpreter::new(bytecode, NoopDb, Env::default(), MAX_GAS);
        let result = interp.run();
        prop_assert!(result.gas_used <= MAX_GAS,
            "Gas accounting error: used {} > limit {}", result.gas_used, MAX_GAS);
    }

    /// Property: execution always terminates (gas enforces termination)
    #[test]
    fn prop_execution_always_terminates(bytecode in prop::collection::vec(any::<u8>(), 0..=256usize)) {
        let mut bytecode = bytecode;
        bytecode.push(0x00); // STOP
        let mut interp = Interpreter::new(bytecode, NoopDb, Env::default(), MAX_GAS);
        let _ = interp.run(); // must return, never block
    }

    /// Property: ZVM native opcodes (0xC0–0xC9) with any stack values never panic
    #[test]
    fn prop_native_opcodes_no_panic(
        opcode_offset in 0u8..10u8,
        seed_a in any::<[u8; 32]>(),
        seed_b in any::<[u8; 32]>(),
    ) {
        let opcode = 0xC0 + opcode_offset;
        let mut bytecode: Vec<u8> = Vec::new();
        // Push 2 stack items
        bytecode.push(0x7f); bytecode.extend_from_slice(&seed_a); // PUSH32
        bytecode.push(0x7f); bytecode.extend_from_slice(&seed_b); // PUSH32
        // Native opcode
        bytecode.push(opcode);
        // POP result + STOP
        bytecode.push(0x50);
        bytecode.push(0x00);

        let mut interp = Interpreter::new(bytecode, NoopDb, Env::default(), MAX_GAS);
        let result = interp.run();
        prop_assert!(result.gas_used <= MAX_GAS);
    }

    /// Property: PUSH + POP roundtrip — stack depth returns to original
    #[test]
    fn prop_push_pop_stack_neutral(values in prop::collection::vec(any::<[u8; 32]>(), 1..=8usize)) {
        let mut bytecode: Vec<u8> = Vec::new();
        for v in &values {
            bytecode.push(0x7f); // PUSH32
            bytecode.extend_from_slice(v);
        }
        for _ in &values {
            bytecode.push(0x50); // POP
        }
        bytecode.push(0x00); // STOP

        let mut interp = Interpreter::new(bytecode, NoopDb, Env::default(), MAX_GAS);
        let result = interp.run();
        prop_assert!(result.gas_used <= MAX_GAS);
        // Stack should be empty after equal push/pop
        prop_assert_eq!(result.final_stack_depth, 0);
    }

    /// Property: EVM signed arithmetic never underflows Rust primitives
    #[test]
    fn prop_signed_arithmetic_no_rust_panic(
        a in any::<[u8; 32]>(),
        b in any::<[u8; 32]>(),
        op in prop_oneof![
            Just(0x04u8), // DIV
            Just(0x05u8), // SDIV
            Just(0x06u8), // MOD
            Just(0x07u8), // SMOD
        ],
    ) {
        let mut bytecode: Vec<u8> = Vec::new();
        bytecode.push(0x7f); bytecode.extend_from_slice(&a); // PUSH32 a
        bytecode.push(0x7f); bytecode.extend_from_slice(&b); // PUSH32 b
        bytecode.push(op);                                    // DIV/SDIV/MOD/SMOD
        bytecode.push(0x50);                                  // POP
        bytecode.push(0x00);                                  // STOP

        let mut interp = Interpreter::new(bytecode, NoopDb, Env::default(), MAX_GAS);
        let result = interp.run();
        prop_assert!(result.gas_used <= MAX_GAS);
    }
}