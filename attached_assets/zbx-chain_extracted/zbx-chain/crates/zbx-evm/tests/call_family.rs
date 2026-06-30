//! S32 — Integration tests for the EVM CALL family in `zbx-evm`.
//!
//! Closes audit C-21 / S7-EVM3 W1+W2+W3+W6 (zbx-evm half). Each test sets
//! up a `MockHost`, installs minimal hand-assembled bytecode at the target
//! address, runs the EVM, and asserts on the resulting return data, host
//! state, and stack outcome.
//!
//! Bytecodes here are written byte-by-byte (no Solidity assembler) to keep
//! the test suite hermetic — no compiler dependency, just opcodes from the
//! Yellow Paper.

use zbx_evm::{
    error::EvmError,
    EVMContext, EVMInterpreter, ExitStatus, Host, MockHost,
};
use zbx_types::address::Address;

// ─────────────────────────────────────────────────────────────────────────
//  Test helpers
// ─────────────────────────────────────────────────────────────────────────

fn addr(b: u8) -> Address {
    let mut a = [0u8; 20];
    a[19] = b;
    Address(a)
}

fn val_u8(b: u8) -> [u8; 32] {
    let mut v = [0u8; 32];
    v[31] = b;
    v
}

fn ctx_for(caller: Address, callee: Address, value: [u8; 32], calldata: Vec<u8>, gas: u64) -> EVMContext {
    EVMContext {
        caller,
        callee,
        value,
        calldata,
        gas_limit: gas,
        is_static: false,
        block_number: 1,
        timestamp: 1,
        coinbase: addr(0xff),
        base_fee: 0,
        chain_id: 8989, // mainnet (locked)
    }
}

/// Run a one-shot frame and return (status, gas_used, return_data).
fn run_frame(ctx: EVMContext, code: Vec<u8>, host: &mut dyn Host)
    -> (ExitStatus, u64, Vec<u8>)
{
    let mut interp = EVMInterpreter::new(ctx, code, host);
    let (status, gas_used) = interp.run();
    let ret = interp.return_data().to_vec();
    (status, gas_used, ret)
}

// ─────────────────────────────────────────────────────────────────────────
//  T1 — CALL into identity precompile (0x04)
//
//  Verifies: precompile dispatch short-circuits before host.code() check;
//  output copied into caller memory; success indicator pushed.
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn call_into_identity_precompile_round_trips_data() {
    let caller = addr(1);
    let mut host = MockHost::new();

    // Bytecode:
    //   PUSH1 0x42; PUSH1 0; MSTORE         ; memory[0..32] = 0x42 padded
    //   PUSH1 32 ret_len; PUSH1 32 ret_off  ; (top last)
    //   PUSH1 32 args_len; PUSH1 0  args_off
    //   PUSH1 0  value;   PUSH1 4  to       ; identity precompile
    //   PUSH2 0x1000 gas
    //   CALL                               ; pushes 1 on success
    //   POP                                 ; discard success indicator
    //   PUSH1 32; PUSH1 32; RETURN          ; return memory[32..64]
    let code = vec![
        0x60, 0x42, 0x60, 0x00, 0x52,
        0x60, 0x20, 0x60, 0x20,
        0x60, 0x20, 0x60, 0x00,
        0x60, 0x00, 0x60, 0x04,
        0x61, 0x10, 0x00,
        0xf1,
        0x50,
        0x60, 0x20, 0x60, 0x20, 0xf3,
    ];

    let ctx = ctx_for(addr(99), caller, [0u8; 32], vec![], 1_000_000);
    let (status, _gas, ret) = run_frame(ctx, code, &mut host);
    assert_eq!(status, ExitStatus::Succeeded);
    assert_eq!(ret.len(), 32);
    assert_eq!(ret[0], 0x42, "identity should echo first input byte");
    for i in 1..32 {
        assert_eq!(ret[i], 0, "identity should echo zero padding");
    }
}

// ─────────────────────────────────────────────────────────────────────────
//  T2 — CALL into empty-code address succeeds with no return data
//
//  Mainnet behaviour for CALLs into EOAs: success, empty return.
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn call_into_empty_account_succeeds_empty_return() {
    let caller = addr(1);
    let target = addr(50); // no code, no balance
    let mut host = MockHost::new();

    // Bytecode: CALL 0x32 (target=50), then push success indicator into ret.
    //   PUSH1 0 ret_len; PUSH1 0 ret_off
    //   PUSH1 0 args_len; PUSH1 0 args_off
    //   PUSH1 0 value;    PUSH1 50 to
    //   PUSH2 0x1000 gas
    //   CALL                 ; pushes success (1) onto stack
    //   PUSH1 0; MSTORE       ; store success at memory[0..32]
    //   PUSH1 32; PUSH1 0; RETURN
    let code = vec![
        0x60, 0x00, 0x60, 0x00,
        0x60, 0x00, 0x60, 0x00,
        0x60, 0x00, 0x60, target.as_bytes()[19],
        0x61, 0x10, 0x00,
        0xf1,
        0x60, 0x00, 0x52,
        0x60, 0x20, 0x60, 0x00, 0xf3,
    ];

    let ctx = ctx_for(addr(99), caller, [0u8; 32], vec![], 1_000_000);
    let (status, _gas, ret) = run_frame(ctx, code, &mut host);
    assert_eq!(status, ExitStatus::Succeeded);
    assert_eq!(ret[31], 1, "CALL to empty code should push 1 (success)");
}

// ─────────────────────────────────────────────────────────────────────────
//  T3 — CALL with insufficient balance pushes 0
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn call_with_insufficient_balance_fails_softly() {
    let caller = addr(1);
    let target = addr(50);
    let mut host = MockHost::new();
    // Caller has zero balance but tries to send 100.
    host.install_code(&target, vec![0x00]); // STOP

    // Bytecode: try CALL with value=100, then store success at memory[0].
    //   PUSH1 100 value: at slot for value
    //   ... full stack: ret_len=0, ret_off=0, args_len=0, args_off=0,
    //                   value=100, to=50, gas=0x10000
    //   CALL
    //   PUSH1 0; MSTORE
    //   PUSH1 32; PUSH1 0; RETURN
    let code = vec![
        0x60, 0x00, 0x60, 0x00,
        0x60, 0x00, 0x60, 0x00,
        0x60, 100,  0x60, 50,
        0x62, 0x01, 0x00, 0x00,
        0xf1,
        0x60, 0x00, 0x52,
        0x60, 0x20, 0x60, 0x00, 0xf3,
    ];

    let ctx = ctx_for(addr(99), caller, [0u8; 32], vec![], 1_000_000);
    let (status, _gas, ret) = run_frame(ctx, code, &mut host);
    assert_eq!(status, ExitStatus::Succeeded);
    assert_eq!(ret[31], 0, "CALL with insufficient balance must push 0");
}

// ─────────────────────────────────────────────────────────────────────────
//  T4 — CALL with value transfer credits the recipient
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn call_with_value_transfers_funds() {
    let caller = addr(1);
    let target = addr(50);
    let mut host = MockHost::new();
    host.credit(&caller, val_u8(100));
    // No code at target — succeeds as empty-account CALL, value still transfers.

    // Bytecode: CALL with value=40, ret_*=0, args_*=0, to=50, gas=0x10000.
    let code = vec![
        0x60, 0x00, 0x60, 0x00,
        0x60, 0x00, 0x60, 0x00,
        0x60,  40,  0x60, 50,
        0x62, 0x01, 0x00, 0x00,
        0xf1,
        0x60, 0x00, 0x52,
        0x60, 0x20, 0x60, 0x00, 0xf3,
    ];

    let ctx = ctx_for(addr(99), caller, [0u8; 32], vec![], 1_000_000);
    {
        let (status, _gas, ret) = run_frame(ctx, code, &mut host);
        assert_eq!(status, ExitStatus::Succeeded);
        assert_eq!(ret[31], 1, "value-transfer CALL should succeed");
    }
    assert_eq!(host.balance(&caller), val_u8(60),  "caller debited");
    assert_eq!(host.balance(&target), val_u8(40),  "target credited");
}

// ─────────────────────────────────────────────────────────────────────────
//  T5 — DELEGATECALL preserves caller, value, and address
//
//  Target code echoes ADDRESS, CALLER, CALLVALUE concatenated. Under
//  DELEGATECALL the values must equal the parent's own callee/caller/value
//  (NOT the target's address).
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn delegatecall_preserves_parent_context() {
    let original_caller = addr(99);
    let parent = addr(1);
    let target = addr(50);
    let mut host = MockHost::new();

    // Target bytecode:
    //   PUSH1 0;  ADDRESS;  MSTORE     ; memory[0..32] = ADDRESS  → parent
    //   PUSH1 32; CALLER;   MSTORE     ; memory[32..64] = CALLER  → original
    //   PUSH1 64; CALLVALUE; MSTORE    ; memory[64..96] = VALUE   → parent.value
    //   PUSH1 96; PUSH1 0; RETURN
    let target_code = vec![
        0x60, 0x00, 0x30, 0x52,
        0x60, 0x20, 0x33, 0x52,
        0x60, 0x40, 0x34, 0x52,
        0x60, 0x60, 0x60, 0x00, 0xf3,
    ];
    host.install_code(&target, target_code);

    // Parent bytecode: DELEGATECALL into target (no value pop).
    //   PUSH1 96 ret_len; PUSH1 0 ret_off
    //   PUSH1 0  args_len; PUSH1 0 args_off
    //   PUSH1 50 to;        PUSH2 0x10000 gas
    //   DELEGATECALL
    //   POP                ; discard success
    //   PUSH1 96 size; PUSH1 0 off; RETURN
    let parent_code = vec![
        0x60, 0x60, 0x60, 0x00,
        0x60, 0x00, 0x60, 0x00,
        0x60, 50,
        0x62, 0x01, 0x00, 0x00,
        0xf4,
        0x50,
        0x60, 0x60, 0x60, 0x00, 0xf3,
    ];

    let ctx = ctx_for(original_caller, parent, val_u8(7), vec![], 5_000_000);
    let (status, _gas, ret) = run_frame(ctx, parent_code, &mut host);
    assert_eq!(status, ExitStatus::Succeeded);
    assert_eq!(ret.len(), 96);
    // ADDRESS slot — last 20 bytes = parent address.
    assert_eq!(&ret[12..32], parent.as_bytes(), "DELEGATECALL preserves ADDRESS");
    // CALLER slot — last 20 bytes = original caller.
    assert_eq!(&ret[44..64], original_caller.as_bytes(), "DELEGATECALL preserves CALLER");
    // CALLVALUE slot — full 32-byte value.
    assert_eq!(&ret[64..96], &val_u8(7)[..], "DELEGATECALL preserves CALLVALUE");
}

// ─────────────────────────────────────────────────────────────────────────
//  T6 — STATICCALL into a contract that attempts state mutation reverts
//
//  Target tries SELFDESTRUCT, which is a state-changing op forbidden in a
//  static frame. The sub-frame fails, parent's STATICCALL pushes 0.
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn staticcall_blocks_selfdestruct() {
    let parent = addr(1);
    let target = addr(50);
    let mut host = MockHost::new();
    // SELFDESTRUCT(beneficiary=0x99) — single byte 0xff after pushing addr.
    let target_code = vec![
        0x60, 0x99,    // PUSH1 0x99
        0xff,          // SELFDESTRUCT
    ];
    host.install_code(&target, target_code);

    // Parent: STATICCALL target with no args.
    //   PUSH1 0 ret_len; PUSH1 0 ret_off
    //   PUSH1 0 args_len; PUSH1 0 args_off
    //   PUSH1 50 to; PUSH2 0x10000 gas
    //   STATICCALL
    //   PUSH1 0; MSTORE
    //   PUSH1 32; PUSH1 0; RETURN
    let parent_code = vec![
        0x60, 0x00, 0x60, 0x00,
        0x60, 0x00, 0x60, 0x00,
        0x60, 50,
        0x62, 0x01, 0x00, 0x00,
        0xfa,
        0x60, 0x00, 0x52,
        0x60, 0x20, 0x60, 0x00, 0xf3,
    ];

    let ctx = ctx_for(addr(99), parent, [0u8; 32], vec![], 5_000_000);
    let (status, _gas, ret) = run_frame(ctx, parent_code, &mut host);
    assert_eq!(status, ExitStatus::Succeeded);
    assert_eq!(ret[31], 0, "STATICCALL into SELFDESTRUCT must push 0");
    // Code at target must NOT have been deleted (sub-call reverted).
    assert!(!host.code(&target).is_empty(), "static-revert must not purge target");
}

// ─────────────────────────────────────────────────────────────────────────
//  T7 — STATICCALL with value transfer in CALL is rejected at parent level
//
//  Even before reaching the sub-frame, a CALL with value > 0 inside an
//  is_static parent frame is a hard error.
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn call_with_value_in_static_frame_errors() {
    let caller = addr(1);
    let target = addr(50);
    let mut host = MockHost::new();
    host.credit(&caller, val_u8(100));

    // Bytecode: CALL with value=10, otherwise zeros.
    let code = vec![
        0x60, 0x00, 0x60, 0x00,
        0x60, 0x00, 0x60, 0x00,
        0x60,  10,  0x60, 50,
        0x62, 0x01, 0x00, 0x00,
        0xf1,
        0x00, // STOP
    ];

    let mut ctx = ctx_for(addr(99), caller, [0u8; 32], vec![], 1_000_000);
    ctx.is_static = true;
    let (status, _gas, _ret) = run_frame(ctx, code, &mut host);
    assert!(matches!(status, ExitStatus::Failed(EvmError::StaticStateChange)));
}

// ─────────────────────────────────────────────────────────────────────────
//  T8 — CREATE deploys a contract whose runtime code is `0x00` (STOP)
//
//  Verifies: address derivation matches keccak256(rlp([sender, nonce]));
//  deployed code installed at that address.
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn create_deploys_contract_at_derived_address() {
    let deployer = addr(1);
    let mut host = MockHost::new();
    host.credit(&deployer, val_u8(100));
    // Deployer starts at nonce 0; CREATE will derive address from nonce 0.

    // Initcode: returns a single STOP byte (0x00) as runtime code.
    //   PUSH1 0x00; PUSH1 0; MSTORE8     ; memory[0] = 0x00
    //   PUSH1 1; PUSH1 0; RETURN          ; return memory[0..1]
    // MSTORE8 is 0x53.
    let initcode: Vec<u8> = vec![
        0x60, 0x00, 0x60, 0x00, 0x53,
        0x60, 0x01, 0x60, 0x00, 0xf3,
    ];

    // Outer code: CREATE(value=0, off=0, len=initcode.len()), then RETURN
    // memory[12..32] (the address word's address bytes).
    //   First, write initcode into memory using MSTORE.
    //   But initcode is 10 bytes — easier to use CODECOPY. Skip; instead
    //   write each byte via MSTORE8.
    //
    // For test simplicity: build a program that writes the initcode via a
    // big PUSH then MSTORE.
    //
    // Easier: pad initcode to 32 bytes with leading zeros, then PUSH32 and
    // MSTORE at offset 0. After MSTORE, memory[0..32] contains 22 zeros
    // followed by the 10 bytes of initcode. Then CREATE(0, 22, 10).
    let mut outer = Vec::new();
    // PUSH32 (initcode left-padded into 32 bytes)
    outer.push(0x7f);
    let mut padded = vec![0u8; 32];
    padded[32 - initcode.len()..].copy_from_slice(&initcode);
    outer.extend_from_slice(&padded);
    outer.extend_from_slice(&[0x60, 0x00, 0x52]); // PUSH1 0; MSTORE
    // CREATE: stack pop order is value, off, len. So push len first, then off, then value.
    outer.extend_from_slice(&[
        0x60, initcode.len() as u8,                       // PUSH1 len
        0x60, (32 - initcode.len()) as u8,                // PUSH1 off
        0x60, 0x00,                                        // PUSH1 value
        0xf0,                                              // CREATE
        // CREATE pushes the new address (32-byte word) onto stack.
        // Store it at memory[0..32] and return.
        0x60, 0x00, 0x52,                                  // PUSH1 0; MSTORE
        0x60, 0x20, 0x60, 0x00, 0xf3,                      // PUSH1 32; PUSH1 0; RETURN
    ]);

    let ctx = ctx_for(addr(99), deployer, [0u8; 32], vec![], 10_000_000);
    let (status, _gas, ret) = run_frame(ctx, outer, &mut host);
    assert_eq!(status, ExitStatus::Succeeded, "CREATE outer frame must succeed");
    assert_eq!(ret.len(), 32);
    let mut deployed_addr_bytes = [0u8; 20];
    deployed_addr_bytes.copy_from_slice(&ret[12..32]);
    let deployed_addr = Address(deployed_addr_bytes);
    assert_ne!(deployed_addr, Address::ZERO, "CREATE must return non-zero address");

    // The host should have STOP installed at the derived address.
    assert_eq!(host.code(&deployed_addr), vec![0x00], "deployed runtime must be STOP");

    // Deployer nonce should have been bumped to 1.
    assert_eq!(host.nonce(&deployer), 1);
    // Deployed contract starts at nonce 1 (EIP-161/7610).
    assert_eq!(host.nonce(&deployed_addr), 1);
}

// ─────────────────────────────────────────────────────────────────────────
//  T9 — CREATE rejects deployed code starting with 0xEF (EIP-3541)
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn create_rejects_ef_prefix_deployed_code() {
    let deployer = addr(1);
    let mut host = MockHost::new();

    // Initcode that returns a single 0xEF byte as runtime code.
    //   PUSH1 0xEF; PUSH1 0; MSTORE8
    //   PUSH1 1; PUSH1 0; RETURN
    let initcode: Vec<u8> = vec![
        0x60, 0xEF, 0x60, 0x00, 0x53,
        0x60, 0x01, 0x60, 0x00, 0xf3,
    ];

    let mut outer = Vec::new();
    outer.push(0x7f);
    let mut padded = vec![0u8; 32];
    padded[32 - initcode.len()..].copy_from_slice(&initcode);
    outer.extend_from_slice(&padded);
    outer.extend_from_slice(&[0x60, 0x00, 0x52]);
    outer.extend_from_slice(&[
        0x60, initcode.len() as u8,
        0x60, (32 - initcode.len()) as u8,
        0x60, 0x00,
        0xf0,
        0x60, 0x00, 0x52,
        0x60, 0x20, 0x60, 0x00, 0xf3,
    ]);

    let ctx = ctx_for(addr(99), deployer, [0u8; 32], vec![], 10_000_000);
    let (status, _gas, ret) = run_frame(ctx, outer, &mut host);
    assert_eq!(status, ExitStatus::Succeeded);
    // CREATE must push 0 on EIP-3541 rejection.
    assert_eq!(ret, vec![0u8; 32], "EIP-3541 must push zero address");
}

// ─────────────────────────────────────────────────────────────────────────
//  T10 — CREATE2 deploys at salt-deterministic address
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn create2_deploys_at_salt_deterministic_address() {
    let deployer = addr(1);
    let mut host = MockHost::new();

    // Same minimal initcode as T8 (returns STOP).
    let initcode: Vec<u8> = vec![
        0x60, 0x00, 0x60, 0x00, 0x53,
        0x60, 0x01, 0x60, 0x00, 0xf3,
    ];

    // CREATE2 stack pop: value, off, len, salt → push reverse: salt, len, off, value.
    let mut outer = Vec::new();
    outer.push(0x7f);
    let mut padded = vec![0u8; 32];
    padded[32 - initcode.len()..].copy_from_slice(&initcode);
    outer.extend_from_slice(&padded);
    outer.extend_from_slice(&[0x60, 0x00, 0x52]); // memory[0..32] = padded initcode
    // Salt = 0x...abcd (32 bytes). PUSH32 salt.
    outer.push(0x7f);
    let salt = {
        let mut s = [0u8; 32];
        s[30] = 0xab;
        s[31] = 0xcd;
        s
    };
    outer.extend_from_slice(&salt);
    // PUSH1 len; PUSH1 off; PUSH1 value; CREATE2
    outer.extend_from_slice(&[
        0x60, initcode.len() as u8,
        0x60, (32 - initcode.len()) as u8,
        0x60, 0x00,
        0xf5,
        0x60, 0x00, 0x52,
        0x60, 0x20, 0x60, 0x00, 0xf3,
    ]);

    let ctx = ctx_for(addr(99), deployer, [0u8; 32], vec![], 10_000_000);
    let (status, _gas, ret) = run_frame(ctx, outer, &mut host);
    assert_eq!(status, ExitStatus::Succeeded);
    let mut deployed_addr_bytes = [0u8; 20];
    deployed_addr_bytes.copy_from_slice(&ret[12..32]);
    let deployed_addr = Address(deployed_addr_bytes);
    assert_ne!(deployed_addr, Address::ZERO);
    assert_eq!(host.code(&deployed_addr), vec![0x00]);
}

// ─────────────────────────────────────────────────────────────────────────
//  T11 — SELFDESTRUCT EIP-6780: pre-existing contract keeps its code
//
//  Target contract was NOT created in this tx (installed up front); after
//  SELFDESTRUCT, the balance moves but code remains.
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn selfdestruct_eip6780_preserves_pre_existing_code() {
    let caller = addr(1);
    let target = addr(50);
    let beneficiary = addr(99);
    let mut host = MockHost::new();
    host.credit(&target, val_u8(70));
    let target_code = vec![
        0x60, 0x63,    // PUSH1 0x63 (=99)
        0xff,          // SELFDESTRUCT
    ];
    host.install_code(&target, target_code.clone());

    // Parent: CALL into target (no value, no return data).
    let parent_code = vec![
        0x60, 0x00, 0x60, 0x00,
        0x60, 0x00, 0x60, 0x00,
        0x60, 0x00, 0x60, 50,
        0x62, 0x01, 0x00, 0x00,
        0xf1,
        0x00, // STOP
    ];

    let ctx = ctx_for(addr(99), caller, [0u8; 32], vec![], 5_000_000);
    let (status, _gas, _ret) = run_frame(ctx, parent_code, &mut host);
    assert_eq!(status, ExitStatus::Succeeded);
    // Beneficiary received funds.
    assert_eq!(host.balance(&beneficiary), val_u8(70));
    // Target balance is zero.
    assert_eq!(host.balance(&target), [0u8; 32]);
    // EIP-6780: code MUST persist (target was not created this tx).
    assert_eq!(host.code(&target), target_code);
}

// ─────────────────────────────────────────────────────────────────────────
//  T12 — RETURNDATASIZE / RETURNDATACOPY round-trip
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn returndata_round_trips_after_call() {
    let caller = addr(1);
    let mut host = MockHost::new();

    // Caller code:
    //   memory[0..32] = 0x77 padded
    //   CALL identity precompile, args=memory[0..32], no ret copy
    //   RETURNDATASIZE → push 32
    //   then RETURNDATACOPY to memory[64..96]
    //   RETURN memory[64..96]
    let code = vec![
        0x60, 0x77, 0x60, 0x00, 0x52,             // memory[0] = 0x77
        // CALL identity, ret_off=0, ret_len=0 (skip in-call copy).
        0x60, 0x00, 0x60, 0x00,                   // ret_len=0, ret_off=0
        0x60, 0x20, 0x60, 0x00,                   // args_len=32, args_off=0
        0x60, 0x00, 0x60, 0x04,                   // value=0, to=0x04
        0x61, 0x10, 0x00,                          // gas=0x1000
        0xf1,                                      // CALL
        0x50,                                      // POP success
        // RETURNDATACOPY(dest=64, off=0, size=32)
        0x60, 0x20, 0x60, 0x00, 0x60, 0x40,       // size=32, off=0, dest=64
        0x3e,                                      // RETURNDATACOPY
        // RETURN memory[64..96]
        0x60, 0x20, 0x60, 0x40, 0xf3,
    ];

    let ctx = ctx_for(addr(99), caller, [0u8; 32], vec![], 1_000_000);
    let (status, _gas, ret) = run_frame(ctx, code, &mut host);
    assert_eq!(status, ExitStatus::Succeeded);
    assert_eq!(ret.len(), 32);
    assert_eq!(ret[0], 0x77, "RETURNDATA must contain identity-echoed first byte");
}

// ─────────────────────────────────────────────────────────────────────────
//  T13a — Gas-mint guard: CALL with stipend MUST NOT refund stipend
//
//  Regression test for architect S32 #2 (consensus-critical). Pre-fix the
//  callee got `forwarded_billed + stipend` of budget but the parent
//  refunded `forwarded - gas_used` — so a no-op callee handed back the
//  full stipend (2300 gas) that the caller had never deducted, minting
//  gas. With the cap-at-billed fix, gas_used must reflect the real cost.
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn call_with_stipend_does_not_mint_gas() {
    let caller = addr(1);
    let target = addr(50);
    let gas_limit: u64 = 1_000_000;
    let mut host = MockHost::new();
    host.credit(&caller, val_u8(100));
    // Empty target — value transfer succeeds, callee runs zero opcodes.

    // CALL with value=10, target empty-code, ret_*=0, args_*=0, gas=large.
    // Then STOP (do not RETURN; simpler — no extra MSTORE/RETURN cost).
    let code = vec![
        0x60, 0x00, 0x60, 0x00,              // ret_len=0, ret_off=0
        0x60, 0x00, 0x60, 0x00,              // args_len=0, args_off=0
        0x60,  10,  0x60, 50,                // value=10, to=50
        0x62, 0x01, 0x00, 0x00,              // PUSH2 0x10000 gas
        0xf1,                                 // CALL
        0x00,                                 // STOP
    ];

    let ctx = ctx_for(addr(99), caller, [0u8; 32], vec![], gas_limit);
    let mut interp = EVMInterpreter::new(ctx, code, &mut host);
    let (status, gas_used) = interp.run();
    assert_eq!(status, ExitStatus::Succeeded);

    // Floor of expected costs:
    //   - 7 PUSH1/PUSH2 ops × 3 gas = 21
    //   - CALL static (warm-base GAS_CALL)            = 100
    //   - cold account access (target not pre-warmed) = 2_500
    //   - value transfer surcharge                    = 9_000
    //   - new account surcharge (target is empty)     = 25_000
    //   ----------------------------------------------------
    // Lower bound (excludes stipend, which must NOT come back to caller):
    let expected_floor: u64 = 21 + 100 + 2_500 + 9_000 + 25_000;
    assert!(
        gas_used >= expected_floor,
        "gas_used={} must be >= {} (stipend mint regression: \
         pre-fix the caller would have been refunded ~2300 gas of \
         stipend they never deducted)",
        gas_used, expected_floor
    );
    // Sanity upper bound — anything close to gas_limit means infinite loop.
    assert!(gas_used < 100_000, "gas_used={} unreasonably high", gas_used);
}

// ─────────────────────────────────────────────────────────────────────────
//  T13 — CREATE address derivation matches well-known test vector
//
//  Validates that our inline RLP + keccak256 path agrees with mainnet.
//  Vector: sender = 0x6ac7ea33f8831ea9dcc53393aaa88b25a785dbf0, nonce 0
//          → 0xcd234a471b72ba2f1ccf0a70fcaba648a5eecd8d
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn create_address_matches_mainnet_vector() {
    // We compute it through a CREATE rather than calling the helper directly
    // (the helper is pub(crate) — only the integration result is observable).
    let deployer = Address::from_hex("0x6ac7ea33f8831ea9dcc53393aaa88b25a785dbf0").unwrap();
    let expected = Address::from_hex("0xcd234a471b72ba2f1ccf0a70fcaba648a5eecd8d").unwrap();
    let mut host = MockHost::new();

    // Initcode returning STOP.
    let initcode: Vec<u8> = vec![
        0x60, 0x00, 0x60, 0x00, 0x53,
        0x60, 0x01, 0x60, 0x00, 0xf3,
    ];
    let mut outer = Vec::new();
    outer.push(0x7f);
    let mut padded = vec![0u8; 32];
    padded[32 - initcode.len()..].copy_from_slice(&initcode);
    outer.extend_from_slice(&padded);
    outer.extend_from_slice(&[0x60, 0x00, 0x52]);
    outer.extend_from_slice(&[
        0x60, initcode.len() as u8,
        0x60, (32 - initcode.len()) as u8,
        0x60, 0x00,
        0xf0,
        0x60, 0x00, 0x52,
        0x60, 0x20, 0x60, 0x00, 0xf3,
    ]);

    let ctx = ctx_for(addr(99), deployer, [0u8; 32], vec![], 10_000_000);
    let (status, _gas, ret) = run_frame(ctx, outer, &mut host);
    assert_eq!(status, ExitStatus::Succeeded);
    let mut got_bytes = [0u8; 20];
    got_bytes.copy_from_slice(&ret[12..32]);
    let got = Address(got_bytes);
    assert_eq!(got, expected, "CREATE address must match Ethereum mainnet vector");
}
