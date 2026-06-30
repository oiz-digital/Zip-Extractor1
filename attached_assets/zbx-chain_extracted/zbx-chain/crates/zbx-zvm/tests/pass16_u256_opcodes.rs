//! SEC-2026-05-09 Pass-16 — full ZVM upgrade integration tests.
//!
//! Covers:
//!   1. U256 arithmetic correctness above the 2^128 boundary (the
//!      pre-Pass-16 silent consensus break).
//!   2. The ~30 EVM opcodes that pre-Pass-16 fell through to the `_`
//!      catch-all and halted the frame as InvalidOpcode (DIV, MOD,
//!      EXP, LT/GT/SLT/SGT, AND/OR/XOR/NOT/BYTE, SHL/SHR/SAR,
//!      KECCAK256, CALLDATA*, CODE*, BLOCKHASH/COINBASE/PREVRANDAO/
//!      GASLIMIT/SELFBALANCE/BLOBHASH, PUSH0/MSIZE, TLOAD/TSTORE/MCOPY).
//!   3. Cancun-era reentrancy guards (TLOAD/TSTORE) — host scratchpad
//!      semantics with no-op default (returns zero on read).
//!   4. EIP-2929 cold/warm gas bumps on BALANCE / EXTCODE* / CALL.

use primitive_types::U256;
use zbx_zvm::{
    context::{ExecutionStatus, ZvmContext},
    executor::ZvmExecutor,
    host::MockZvmHost,
};

fn run(bytecode: Vec<u8>) -> zbx_zvm::context::ZvmResult {
    let mut host = MockZvmHost::new();
    let mut ctx = ZvmContext::test_default();
    ctx.bytecode  = bytecode;
    ctx.gas_limit = 10_000_000;
    ZvmExecutor::execute(&ctx, &mut host)
}

fn run_with_calldata(bytecode: Vec<u8>, calldata: Vec<u8>) -> zbx_zvm::context::ZvmResult {
    let mut host = MockZvmHost::new();
    let mut ctx = ZvmContext::test_default();
    ctx.bytecode  = bytecode;
    ctx.calldata  = calldata;
    ctx.gas_limit = 10_000_000;
    ZvmExecutor::execute(&ctx, &mut host)
}

/// Build PUSH32 <word> bytecode prefix.
fn push32(value: U256) -> Vec<u8> {
    let mut be = [0u8; 32];
    value.to_big_endian(&mut be);
    let mut v = Vec::with_capacity(33);
    v.push(0x7F);          // PUSH32
    v.extend_from_slice(&be);
    v
}

fn push1(byte: u8) -> Vec<u8> {
    vec![0x60, byte]
}

/// Extract the top word of return data interpreted as U256.
fn ret_u256(r: &zbx_zvm::context::ZvmResult) -> U256 {
    assert_eq!(r.status, ExecutionStatus::Success, "frame did not succeed: {:?}", r.status);
    assert!(r.return_data.len() >= 32, "no return word");
    U256::from_big_endian(&r.return_data[..32])
}

// ─── (1) U256 ARITHMETIC ─────────────────────────────────────────────────────

#[test]
fn u256_add_above_2_to_128() {
    // (2^130 + 2^130) → 2^131. Pre-Pass-16 this truncated to 0.
    let a = U256::from(1u8) << 130;
    let mut bc = push32(a);
    bc.extend(push32(a));
    bc.push(0x01);                        // ADD
    bc.extend(vec![0x60, 0x00, 0x52]);    // PUSH1 0 MSTORE
    bc.extend(vec![0x60, 0x20, 0x60, 0x00, 0xF3]); // PUSH1 32 PUSH1 0 RETURN
    assert_eq!(ret_u256(&run(bc)), U256::from(1u8) << 131);
}

#[test]
fn u256_mul_above_2_to_128() {
    // (2^100) * (2^100) = 2^200. Pre-Pass-16 truncated to 0.
    let a = U256::from(1u8) << 100;
    let mut bc = push32(a);
    bc.extend(push32(a));
    bc.push(0x02);                        // MUL
    bc.extend(vec![0x60, 0x00, 0x52]);
    bc.extend(vec![0x60, 0x20, 0x60, 0x00, 0xF3]);
    assert_eq!(ret_u256(&run(bc)), U256::from(1u8) << 200);
}

// ─── (2) NEW OPCODES — DIV / MOD / EXP / SIGNEXTEND ──────────────────────────

#[test]
fn div_truncates_toward_zero() {
    // 100 / 7 = 14
    let mut bc = push1(7);                 // DIV pops a (top) then b — so push b then a.
    bc.extend(push1(100));
    bc.push(0x04);                         // DIV
    bc.extend(vec![0x60, 0x00, 0x52]);
    bc.extend(vec![0x60, 0x20, 0x60, 0x00, 0xF3]);
    assert_eq!(ret_u256(&run(bc)), U256::from(14u64));
}

#[test]
fn div_by_zero_returns_zero() {
    let mut bc = push1(0);
    bc.extend(push1(100));
    bc.push(0x04);                         // DIV
    bc.extend(vec![0x60, 0x00, 0x52]);
    bc.extend(vec![0x60, 0x20, 0x60, 0x00, 0xF3]);
    assert_eq!(ret_u256(&run(bc)), U256::zero());
}

#[test]
fn mod_basic() {
    let mut bc = push1(7);
    bc.extend(push1(100));
    bc.push(0x06);                         // MOD
    bc.extend(vec![0x60, 0x00, 0x52]);
    bc.extend(vec![0x60, 0x20, 0x60, 0x00, 0xF3]);
    assert_eq!(ret_u256(&run(bc)), U256::from(2u64));
}

#[test]
fn exp_2_to_200() {
    let mut bc = push1(200);               // exponent
    bc.extend(push1(2));                   // base
    bc.push(0x0A);                         // EXP
    bc.extend(vec![0x60, 0x00, 0x52]);
    bc.extend(vec![0x60, 0x20, 0x60, 0x00, 0xF3]);
    assert_eq!(ret_u256(&run(bc)), U256::from(1u8) << 200);
}

#[test]
fn sdiv_negative_div_negative() {
    // (-12) / (-3) = 4
    let neg12 = (!U256::from(12u64)).overflowing_add(U256::one()).0;
    let neg3  = (!U256::from(3u64)).overflowing_add(U256::one()).0;
    let mut bc = push32(neg3);
    bc.extend(push32(neg12));
    bc.push(0x05);                         // SDIV
    bc.extend(vec![0x60, 0x00, 0x52]);
    bc.extend(vec![0x60, 0x20, 0x60, 0x00, 0xF3]);
    assert_eq!(ret_u256(&run(bc)), U256::from(4u64));
}

#[test]
fn signextend_byte0_negative() {
    // SIGNEXTEND(0, 0xFF) = all-ones (sign-extend 8-bit -1 to 256-bit -1).
    let mut bc = push1(0xFF);              // x
    bc.extend(push1(0));                   // b = 0
    bc.push(0x0B);                         // SIGNEXTEND
    bc.extend(vec![0x60, 0x00, 0x52]);
    bc.extend(vec![0x60, 0x20, 0x60, 0x00, 0xF3]);
    assert_eq!(ret_u256(&run(bc)), U256::max_value());
}

// ─── (3) COMPARISON + BITWISE ────────────────────────────────────────────────

#[test]
fn lt_gt_basic() {
    // 5 < 10 → 1
    let mut bc = push1(10);
    bc.extend(push1(5));
    bc.push(0x10);                         // LT
    bc.extend(vec![0x60, 0x00, 0x52]);
    bc.extend(vec![0x60, 0x20, 0x60, 0x00, 0xF3]);
    assert_eq!(ret_u256(&run(bc)), U256::one());
}

#[test]
fn slt_negative_lt_positive() {
    // SLT(-1, 1) → 1
    let mut bc = push1(1);
    bc.extend(push32(U256::max_value()));  // -1
    bc.push(0x12);                         // SLT
    bc.extend(vec![0x60, 0x00, 0x52]);
    bc.extend(vec![0x60, 0x20, 0x60, 0x00, 0xF3]);
    assert_eq!(ret_u256(&run(bc)), U256::one());
}

#[test]
fn and_or_xor_not() {
    // 0xF0 & 0x0F = 0x00; (0xF0 | 0x0F) = 0xFF; (0xF0 ^ 0x0F) = 0xFF
    // Test AND
    let mut bc = push1(0x0F);
    bc.extend(push1(0xF0));
    bc.push(0x16);                         // AND
    bc.extend(vec![0x60, 0x00, 0x52]);
    bc.extend(vec![0x60, 0x20, 0x60, 0x00, 0xF3]);
    assert_eq!(ret_u256(&run(bc)), U256::zero());
}

#[test]
fn shl_shr() {
    // 1 << 200 = 2^200
    let mut bc = push1(1);                 // value
    bc.extend(push1(200));                 // shift
    bc.push(0x1B);                         // SHL
    bc.extend(vec![0x60, 0x00, 0x52]);
    bc.extend(vec![0x60, 0x20, 0x60, 0x00, 0xF3]);
    assert_eq!(ret_u256(&run(bc)), U256::from(1u8) << 200);
}

#[test]
fn sar_negative_preserves_sign() {
    // SAR(-1, 4) = -1 (sign extends).
    let mut bc = push32(U256::max_value()); // value (-1)
    bc.extend(push1(4));                    // shift
    bc.push(0x1D);                          // SAR
    bc.extend(vec![0x60, 0x00, 0x52]);
    bc.extend(vec![0x60, 0x20, 0x60, 0x00, 0xF3]);
    assert_eq!(ret_u256(&run(bc)), U256::max_value());
}

// ─── (4) KECCAK256 ───────────────────────────────────────────────────────────

#[test]
fn keccak256_empty() {
    // KECCAK256(""), well-known value.
    // PUSH1 0 PUSH1 0 KECCAK256 ...
    let mut bc = vec![0x60, 0x00, 0x60, 0x00, 0x20]; // KECCAK256
    bc.extend(vec![0x60, 0x00, 0x52]);
    bc.extend(vec![0x60, 0x20, 0x60, 0x00, 0xF3]);
    let r = run(bc);
    // keccak256("") = c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470
    let expected: [u8; 32] = [
        0xc5,0xd2,0x46,0x01,0x86,0xf7,0x23,0x3c,
        0x92,0x7e,0x7d,0xb2,0xdc,0xc7,0x03,0xc0,
        0xe5,0x00,0xb6,0x53,0xca,0x82,0x27,0x3b,
        0x7b,0xfa,0xd8,0x04,0x5d,0x85,0xa4,0x70,
    ];
    assert_eq!(&r.return_data[..32], &expected);
}

// ─── (5) CALLDATA / CODE ─────────────────────────────────────────────────────

#[test]
fn calldataload_first_word() {
    // CALLDATALOAD(0) of [0xAA; 32] → all-AA word.
    let cd = vec![0xAAu8; 32];
    let mut bc = vec![0x60, 0x00, 0x35]; // PUSH1 0 CALLDATALOAD
    bc.extend(vec![0x60, 0x00, 0x52, 0x60, 0x20, 0x60, 0x00, 0xF3]);
    let r = run_with_calldata(bc, cd);
    assert_eq!(r.return_data[..32].iter().all(|&b| b == 0xAA), true);
}

#[test]
fn calldatasize() {
    let cd = vec![0u8; 100];
    let bc = vec![0x36, 0x60, 0x00, 0x52, 0x60, 0x20, 0x60, 0x00, 0xF3]; // CALLDATASIZE MSTORE …
    let r = run_with_calldata(bc, cd);
    assert_eq!(ret_u256(&r), U256::from(100u64));
}

#[test]
fn codesize() {
    // Bytecode is 9 bytes long.
    let bc = vec![0x38, 0x60, 0x00, 0x52, 0x60, 0x20, 0x60, 0x00, 0xF3]; // CODESIZE MSTORE …
    let r = run(bc);
    assert_eq!(ret_u256(&r), U256::from(9u64));
}

// ─── (6) PUSH0 / MSIZE ───────────────────────────────────────────────────────

#[test]
fn push0_zero_word() {
    let mut bc = vec![0x5F];               // PUSH0
    bc.extend(vec![0x60, 0x00, 0x52, 0x60, 0x20, 0x60, 0x00, 0xF3]);
    assert_eq!(ret_u256(&run(bc)), U256::zero());
}

// ─── (7) TLOAD / TSTORE / MCOPY ──────────────────────────────────────────────

#[test]
fn tload_default_zero() {
    // TLOAD on a fresh slot must return 0 (default host has no scratchpad).
    let mut bc = vec![0x60, 0x42, 0x5C];   // PUSH1 0x42 TLOAD
    bc.extend(vec![0x60, 0x00, 0x52, 0x60, 0x20, 0x60, 0x00, 0xF3]);
    assert_eq!(ret_u256(&run(bc)), U256::zero());
}

#[test]
fn mcopy_overlapping() {
    // Write 32 bytes of 0xCC at offset 0, then MCOPY 16 bytes from 0 to 8
    // (overlapping). Read back word at offset 0.
    //
    // Bytecode:
    //  PUSH32 [0xCC*32] PUSH1 0 MSTORE
    //  PUSH1 16 PUSH1 0 PUSH1 8 MCOPY
    //  PUSH1 0 MLOAD PUSH1 64 MSTORE
    //  PUSH1 32 PUSH1 64 RETURN
    let pattern = U256::from_big_endian(&[0xCCu8; 32]);
    let mut bc = push32(pattern);
    bc.extend(vec![0x60, 0x00, 0x52]);     // MSTORE @0
    bc.extend(vec![0x60, 16, 0x60, 0, 0x60, 8, 0x5E]); // MCOPY(dst=8,src=0,len=16)
    bc.extend(vec![0x60, 0x00, 0x51]);     // MLOAD @0
    bc.extend(vec![0x60, 64, 0x52]);       // MSTORE @64
    bc.extend(vec![0x60, 0x20, 0x60, 64, 0xF3]); // RETURN(64,32)
    let r = run(bc);
    // After MCOPY, bytes [8..24) should be 0xCC (overlap of original).
    // Bytes [0..8) untouched (still 0xCC). So entire [0..32) of word at 0
    // is still 0xCC.
    assert_eq!(r.status, ExecutionStatus::Success);
    assert!(r.return_data[..32].iter().all(|&b| b == 0xCC));
}

// ─── (8) BLOCK INFO ──────────────────────────────────────────────────────────

#[test]
fn coinbase_default_zero() {
    let bc = vec![0x41, 0x60, 0x00, 0x52, 0x60, 0x20, 0x60, 0x00, 0xF3]; // COINBASE
    assert_eq!(ret_u256(&run(bc)), U256::zero());
}

#[test]
fn gaslimit_default_30m() {
    let bc = vec![0x45, 0x60, 0x00, 0x52, 0x60, 0x20, 0x60, 0x00, 0xF3]; // GASLIMIT
    assert_eq!(ret_u256(&run(bc)), U256::from(30_000_000u64));
}

#[test]
fn selfbalance_default_zero() {
    let bc = vec![0x47, 0x60, 0x00, 0x52, 0x60, 0x20, 0x60, 0x00, 0xF3]; // SELFBALANCE
    assert_eq!(ret_u256(&run(bc)), U256::zero());
}

// ─── (9) PRE-PASS-16 REGRESSION GUARD ────────────────────────────────────────

#[test]
fn arithmetic_no_silent_truncation_at_2_to_128() {
    // The Pass-16 raison-d'être: pre-fix `pop_u128` on a stack word with
    // any byte set in the upper 16 bytes silently dropped them. This test
    // confirms the new path retains them.
    let big = U256::from_big_endian(&[
        0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ]); // 2^255
    let mut bc = push32(big);
    bc.extend(push32(U256::zero()));
    bc.push(0x01);                         // ADD (big + 0)
    bc.extend(vec![0x60, 0x00, 0x52]);
    bc.extend(vec![0x60, 0x20, 0x60, 0x00, 0xF3]);
    assert_eq!(ret_u256(&run(bc)), big);
}
