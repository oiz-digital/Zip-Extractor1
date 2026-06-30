//! Property-based fuzz: structured opcode sequences with proptest.
//! Uses Arbitrary instead of raw bytes so coverage is more targeted.
//!
//! Run:
//!   cargo +nightly fuzz run fuzz_zvm_opcodes -- -max_total_time=120
#![no_main]

use libfuzzer_sys::{fuzz_target, arbitrary};
use arbitrary::Arbitrary;
use zbx_zvm::{Interpreter, state::NoopDb, env::Env};

#[derive(Arbitrary, Debug, Clone, Copy)]
#[repr(u8)]
enum SimpleOpcode {
    // Arithmetic
    Stop  = 0x00, Add   = 0x01, Mul   = 0x02, Sub   = 0x03,
    Div   = 0x04, Mod   = 0x06, Exp   = 0x0a,
    // Comparison
    Lt    = 0x10, Gt    = 0x11, Eq    = 0x14, IsZero= 0x15,
    And   = 0x16, Or    = 0x17, Xor   = 0x18, Not   = 0x19,
    Shl   = 0x1b, Shr   = 0x1c, Sar   = 0x1d,
    // Memory
    MLoad = 0x51, MStore= 0x52, MStore8=0x53,
    // Stack
    Pop   = 0x50, Dup1  = 0x80, Swap1 = 0x90,
    // ZVM native
    PayId    = 0xC0, ZusdBal  = 0xC1, ZbxPrice = 0xC2,
    ZbxTime  = 0xC3, AaSender = 0xC4, ChainVer = 0xC5,
    BlobFee  = 0xC6, PayIdSet = 0xC7, ZbxBurn  = 0xC8, ZvmLog = 0xC9,
    // Control
    JumpDest= 0x5b, Jump = 0x56, JumpI= 0x57,
    // System
    Return  = 0xf3, Revert= 0xfd, Invalid= 0xfe,
}

#[derive(Arbitrary, Debug)]
struct OpSequence {
    /// Stack seed values (up to 4 push32 values)
    seeds: [[u8; 32]; 4],
    /// Sequence of opcodes to execute after pushing seeds
    ops: Vec<SimpleOpcode>,
}

fuzz_target!(|seq: OpSequence| {
    const GAS: u64 = 500_000;

    let mut bytecode: Vec<u8> = Vec::new();

    // Push seed values
    for seed in &seq.seeds {
        bytecode.push(0x7f); // PUSH32
        bytecode.extend_from_slice(seed);
    }

    // Apply opcode sequence (max 32 ops for fast execution)
    for op in seq.ops.iter().take(32) {
        let byte = *op as u8;
        // JUMP/JUMPI targets must be valid JUMPDESTs — skip them in raw form
        if byte == 0x56 || byte == 0x57 { continue; }
        bytecode.push(byte);
    }

    // Always end with STOP
    bytecode.push(0x00);

    let mut interp = Interpreter::new(bytecode, NoopDb, Env::default(), GAS);
    let result = interp.run();

    // Invariant: gas accounting
    assert!(result.gas_used <= GAS,
        "Gas leak detected: used {} > limit {}", result.gas_used, GAS);
});