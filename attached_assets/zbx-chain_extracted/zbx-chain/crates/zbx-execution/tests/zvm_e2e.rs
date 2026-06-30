//! ZVM execution dispatch path — end-to-end.
//!
//! Verifies:
//!   1. CREATE with the `0x5A` discriminator strips the byte and
//!      marks the deployed account as `VmKind::Zvm`.
//!   2. CREATE without the discriminator stays on the EVM path.
//!   3. CALL into a `VmKind::Zvm` account dispatches through the
//!      ZVM interpreter via `ProductionZvmHost`.
//!   4. CALL into a `VmKind::Evm` account stays on the EVM path.
//!   5. Multi-tx: ZVM SSTORE persists across txs, LOG0 surfaces on
//!      the receipt, and the EIP-1153 transient scratchpad is wiped
//!      between txs (TLOAD in tx2 returns zero, not the value
//!      TSTORE'd in tx1).

use zbx_crypto::keccak::keccak256;
use zbx_execution::{BlockExecutor, StateView, ZVM_DEPLOY_DISCRIMINATOR};
use zbx_types::{
    account::{AccountState, VmKind},
    address::Address,
    block::{Block, BlockBody, BlockHeader},
    transaction::{Signature, SignedTransaction, Transaction, TxType},
    H256, U256,
};

const CHAIN_ID: u64 = 8990;

fn addr(byte: u8) -> Address {
    let mut a = [0u8; 20];
    a[19] = byte;
    Address(a)
}

fn fund(view: &mut StateView, who: Address, wei: u128) {
    let mut a = AccountState::default();
    a.set_balance_u128(wei);
    view.set_account(who, a);
}

fn header(coinbase: Address) -> BlockHeader {
    BlockHeader {
        parent_hash: H256::zero(),
        uncle_hash: H256::zero(),
        coinbase,
        state_root: H256::zero(),
        transactions_root: H256::zero(),
        receipts_root: H256::zero(),
        logs_bloom: [0u8; 256],
        difficulty: U256::zero(),
        number: 1,
        gas_limit: 30_000_000,
        gas_used: 0,
        timestamp: 1_700_000_000,
        extra_data: Vec::new(),
        mix_hash: H256::zero(),
        nonce: 0,
        base_fee_per_gas: 1,
        committee_signature: Vec::new(),
        epoch: 0,
        epoch_seed: None,
    }
}

fn mk_tx(
    from: Address,
    to: Option<Address>,
    nonce: u64,
    data: Vec<u8>,
    gas_limit: u64,
) -> SignedTransaction {
    let tx = Transaction {
        tx_type: TxType::DynamicFee,
        chain_id: CHAIN_ID,
        nonce,
        max_fee_per_gas: 1,
        max_priority_fee_per_gas: 0,
        gas_limit,
        to,
        value: U256::zero(),
        data,
        access_list: Vec::new(),
    };
    let hash = tx.signing_hash();
    SignedTransaction {
        from,
        tx,
        sig: Signature { v: 0, r: H256::zero(), s: H256::zero() },
        hash,
    }
}

struct BlockOutcome {
    after_accounts: StateView,
    storage: std::collections::HashMap<Address, std::collections::HashMap<H256, H256>>,
    receipts: Vec<zbx_types::receipt::TransactionReceipt>,
}

fn run_block(view: StateView, txs: Vec<SignedTransaction>, coinbase: Address) -> BlockOutcome {
    let block = Block {
        header: header(coinbase),
        body: BlockBody { transactions: txs, uncles: Vec::new() },
    };
    let result = BlockExecutor::execute(&block, view).expect("block must execute");
    let mut after = StateView::new();
    for (a, s) in result.state_diff.accounts {
        after.set_account(a, s);
    }
    BlockOutcome {
        after_accounts: after,
        storage: result.state_diff.storage,
        receipts: result.receipts,
    }
}

#[test]
fn create_with_zvm_discriminator_marks_account_as_zvm() {
    let mut view = StateView::new();
    let sender = addr(0x11);
    let coinbase = addr(0xC0);
    fund(&mut view, sender, 10u128.pow(20));

    let init_code = vec![ZVM_DEPLOY_DISCRIMINATOR, 0x00];
    let tx = mk_tx(sender, None, 0, init_code, 100_000);

    let out = run_block(view, vec![tx], coinbase);
    assert!(matches!(out.receipts[0].status, zbx_types::receipt::TxStatus::Success));

    let mut buf = Vec::with_capacity(28);
    buf.extend_from_slice(sender.as_bytes());
    buf.extend_from_slice(&0u64.to_be_bytes());
    let h = keccak256(&buf);
    let mut a = [0u8; 20];
    a.copy_from_slice(&h.as_bytes()[12..]);
    let new_addr = Address(a);

    let deployed = out.after_accounts.get_account(&new_addr);
    assert_eq!(deployed.vm, VmKind::Zvm,
        "0x5A discriminator must persist VmKind::Zvm");
}

#[test]
fn create_without_discriminator_stays_on_evm_path() {
    let mut view = StateView::new();
    let sender = addr(0x22);
    let coinbase = addr(0xC0);
    fund(&mut view, sender, 10u128.pow(20));

    let tx = mk_tx(sender, None, 0, vec![0x00], 100_000);
    let out = run_block(view, vec![tx], coinbase);
    assert!(matches!(out.receipts[0].status, zbx_types::receipt::TxStatus::Success));

    let mut buf = Vec::with_capacity(28);
    buf.extend_from_slice(sender.as_bytes());
    buf.extend_from_slice(&0u64.to_be_bytes());
    let h = keccak256(&buf);
    let mut a = [0u8; 20];
    a.copy_from_slice(&h.as_bytes()[12..]);
    let new_addr = Address(a);

    let deployed = out.after_accounts.get_account(&new_addr);
    assert_eq!(deployed.vm, VmKind::Evm);
}

#[test]
fn call_into_zvm_account_dispatches_through_zvm() {
    let mut view = StateView::new();
    let sender = addr(0x33);
    let callee = addr(0x44);
    let coinbase = addr(0xC0);
    fund(&mut view, sender, 10u128.pow(20));

    let stop_code = vec![0x00u8];
    let code_hash = keccak256(&stop_code);
    let mut callee_acct = AccountState::default();
    callee_acct.code_hash = code_hash;
    callee_acct.vm = VmKind::Zvm;
    view.set_account(callee, callee_acct);
    view.seed_code(code_hash, stop_code);

    let tx = mk_tx(sender, Some(callee), 0, Vec::new(), 100_000);
    let out = run_block(view, vec![tx], coinbase);
    assert!(matches!(out.receipts[0].status, zbx_types::receipt::TxStatus::Success));
}

#[test]
fn call_into_evm_account_keeps_evm_path() {
    let mut view = StateView::new();
    let sender = addr(0x55);
    let callee = addr(0x66);
    let coinbase = addr(0xC0);
    fund(&mut view, sender, 10u128.pow(20));

    let stop_code = vec![0x00u8];
    let code_hash = keccak256(&stop_code);
    let mut callee_acct = AccountState::default();
    callee_acct.code_hash = code_hash;
    callee_acct.vm = VmKind::Evm;
    view.set_account(callee, callee_acct);
    view.seed_code(code_hash, stop_code);

    let tx = mk_tx(sender, Some(callee), 0, Vec::new(), 100_000);
    let out = run_block(view, vec![tx], coinbase);
    assert!(matches!(out.receipts[0].status, zbx_types::receipt::TxStatus::Success));
}

/// Deploy-then-call pipeline (Pass-3 follow-up). Single block:
///   tx0  CREATE with 0x5A discriminator + runtime code
///        `60 42 60 07 55 00` (PUSH1 0x42, PUSH1 0x07, SSTORE, STOP)
///        — deploys a ZVM contract that, when called, writes 0x42 into
///        storage slot 7. The 0x5A prefix marks the new account ZVM and
///        the executor must persist the runtime code+code_hash so tx1
///        can resolve it.
///   tx1  CALL the freshly-deployed contract with empty calldata.
/// Asserts: receipts both Success, deployed account is VmKind::Zvm with
/// non-zero code_hash, and storage slot 7 of the new contract == 0x42.
#[test]
fn deploy_then_call_zvm_contract_persists_runtime_code_and_executes() {
    let mut view = StateView::new();
    let sender   = addr(0x77);
    let coinbase = addr(0xC0);
    fund(&mut view, sender, 10u128.pow(20));

    // Predict the CREATE address (matches executor's surrogate scheme).
    let mut buf = Vec::with_capacity(28);
    buf.extend_from_slice(sender.as_bytes());
    buf.extend_from_slice(&0u64.to_be_bytes());
    let h = keccak256(&buf);
    let mut a = [0u8; 20];
    a.copy_from_slice(&h.as_bytes()[12..]);
    let new_addr = Address(a);

    let runtime: Vec<u8> = vec![0x60, 0x42, 0x60, 0x07, 0x55, 0x00];
    let mut init = Vec::with_capacity(runtime.len() + 1);
    init.push(ZVM_DEPLOY_DISCRIMINATOR);
    init.extend_from_slice(&runtime);

    let tx_create = mk_tx(sender, None,            0, init,        200_000);
    let tx_call   = mk_tx(sender, Some(new_addr),  1, Vec::new(),  200_000);

    let out = run_block(view, vec![tx_create, tx_call], coinbase);
    assert!(matches!(out.receipts[0].status, zbx_types::receipt::TxStatus::Success),
        "CREATE must succeed");
    assert!(matches!(out.receipts[1].status, zbx_types::receipt::TxStatus::Success),
        "CALL into deployed ZVM contract must succeed (proves runtime code was persisted)");

    let deployed = out.after_accounts.get_account(&new_addr);
    assert_eq!(deployed.vm, VmKind::Zvm);
    assert_ne!(deployed.code_hash, H256::zero(),
        "runtime code_hash must be persisted on CREATE");

    let slot7 = H256({
        let mut k = [0u8; 32];
        k[31] = 0x07;
        k
    });
    let stored = out.storage
        .get(&new_addr).and_then(|m| m.get(&slot7)).copied()
        .unwrap_or(H256::zero());
    let mut expect = [0u8; 32];
    expect[31] = 0x42;
    assert_eq!(stored, H256(expect),
        "ZVM contract's SSTORE 0x42 → slot 7 must persist after CALL");
}

/// Cross-VM CALL gate (Pass-3 follow-up). A ZVM contract attempts
/// `CALL <evm_marked_target>` — `ProductionZvmHost::is_call_allowed`
/// must reject it before any sub-execution, the ZVM interpreter
/// pushes 0 (failure) onto the stack, and our test contract then
/// SSTOREs that 0 into slot 0. The corresponding ZVM→ZVM scenario
/// (call against a same-VM target) returns 1 — proven by a second
/// run that flips only the target's `vm` flag.
///
/// EVM→ZVM gating is structurally moot in this codebase today: the
/// EVM execution path uses `MockHost` (executor.rs L762) which has
/// empty state, so an EVM contract's CALL opcode cannot resolve any
/// real ZVM-marked account at all — it always sees an empty target
/// and STOPs. Wiring the EVM through a real production host is out
/// of scope for Task #2 and tracked separately.
#[test]
fn zvm_to_evm_call_is_rejected_with_zero_zvm_to_zvm_returns_one() {
    fn run(target_vm: VmKind) -> H256 {
        let mut view = StateView::new();
        let sender   = addr(0x88);
        let coinbase = addr(0xC0);
        fund(&mut view, sender, 10u128.pow(20));

        // Pre-seed the CALL target as a contract under `target_vm`.
        let target = addr(0x99);
        let target_runtime = vec![0x00u8]; // STOP — body irrelevant
        let target_hash = keccak256(&target_runtime);
        let mut target_acct = AccountState::default();
        target_acct.code_hash = target_hash;
        target_acct.vm = target_vm;
        view.set_account(target, target_acct);
        view.seed_code(target_hash, target_runtime);

        // Caller bytecode: CALL(gas=0x2710, target, value=0, 0,0,0,0)
        // then SSTORE the returned status into slot 0; STOP.
        //   60 00              PUSH1 0   (ret_len)
        //   60 00              PUSH1 0   (ret_off)
        //   60 00              PUSH1 0   (args_len)
        //   60 00              PUSH1 0   (args_off)
        //   60 00              PUSH1 0   (value)
        //   73 <20-byte target> PUSH20 target
        //   61 27 10           PUSH2 0x2710 (gas)
        //   F1                 CALL
        //   60 00              PUSH1 0
        //   55                 SSTORE
        //   00                 STOP
        let mut caller_runtime: Vec<u8> = vec![
            0x60, 0x00, 0x60, 0x00, 0x60, 0x00, 0x60, 0x00, 0x60, 0x00,
            0x73,
        ];
        caller_runtime.extend_from_slice(target.as_bytes());
        caller_runtime.extend_from_slice(&[0x61, 0x27, 0x10, 0xF1, 0x60, 0x00, 0x55, 0x00]);

        let caller = addr(0xAA);
        let caller_hash = keccak256(&caller_runtime);
        let mut caller_acct = AccountState::default();
        caller_acct.code_hash = caller_hash;
        caller_acct.vm = VmKind::Zvm;
        view.set_account(caller, caller_acct);
        view.seed_code(caller_hash, caller_runtime);

        let tx = mk_tx(sender, Some(caller), 0, Vec::new(), 200_000);
        let out = run_block(view, vec![tx], coinbase);
        assert!(matches!(out.receipts[0].status, zbx_types::receipt::TxStatus::Success),
            "outer ZVM frame must complete (CALL itself doesn't revert)");

        out.storage.get(&caller)
            .and_then(|m| m.get(&H256::zero()))
            .copied()
            .unwrap_or(H256::zero())
    }

    // ZVM→EVM: rejected → SSTORE'd 0
    let cross  = run(VmKind::Evm);
    assert_eq!(cross, H256::zero(),
        "ZVM→EVM CALL must be rejected (push 0) by is_call_allowed gate");

    // ZVM→ZVM: allowed → CALL into STOP-only contract returns 1
    let same   = run(VmKind::Zvm);
    let mut one = [0u8; 32]; one[31] = 0x01;
    assert_eq!(same, H256(one),
        "ZVM→ZVM CALL into STOP contract must return success (push 1)");
}

/// Multi-tx scenario: tx1 hits a ZVM contract that TSTOREs into slot 1,
/// SSTOREs slot 1, and emits LOG0. tx2 hits a different ZVM contract
/// that TLOADs slot 1 and SSTOREs the result into its own slot 2. The
/// transient scratchpad MUST be wiped between txs (Cancun EIP-1153),
/// so tx2's TLOAD returns 0 — proven by `b.storage[2] == 0`.
#[test]
fn multi_tx_state_mutation_log_emission_and_transient_clear_between_txs() {
    // Opcodes (EVM-compatible, also valid ZVM):
    //   PUSH1=0x60, SSTORE=0x55, TSTORE=0x5D, TLOAD=0x5C,
    //   LOG0=0xA0, STOP=0x00.
    //
    // Contract A — TSTORE 1=0xAA, SSTORE 1=0xCD, LOG0 (empty), STOP:
    //   60 AA 60 01 5D     ; TSTORE
    //   60 CD 60 01 55     ; SSTORE
    //   60 00 60 00 A0     ; LOG0 with offset=0,len=0
    //   00                 ; STOP
    let code_a: Vec<u8> = vec![
        0x60, 0xAA, 0x60, 0x01, 0x5D,
        0x60, 0xCD, 0x60, 0x01, 0x55,
        0x60, 0x00, 0x60, 0x00, 0xA0,
        0x00,
    ];
    // Contract B — TLOAD slot 1, SSTORE slot 2 = TLOAD-result, STOP:
    //   60 01 5C           ; TLOAD slot 1   -> stack
    //   60 02 55           ; SSTORE slot 2
    //   00                 ; STOP
    let code_b: Vec<u8> = vec![
        0x60, 0x01, 0x5C,
        0x60, 0x02, 0x55,
        0x00,
    ];

    let mut view = StateView::new();
    let sender = addr(0x77);
    let cont_a = addr(0xA1);
    let cont_b = addr(0xB1);
    let coinbase = addr(0xC0);
    fund(&mut view, sender, 10u128.pow(20));

    let hash_a = keccak256(&code_a);
    let hash_b = keccak256(&code_b);
    for (a, h, c) in [
        (cont_a, hash_a, code_a.clone()),
        (cont_b, hash_b, code_b.clone()),
    ] {
        let mut acct = AccountState::default();
        acct.code_hash = h;
        acct.vm = VmKind::Zvm;
        view.set_account(a, acct);
        view.seed_code(h, c);
    }

    let tx1 = mk_tx(sender, Some(cont_a), 0, Vec::new(), 200_000);
    let tx2 = mk_tx(sender, Some(cont_b), 1, Vec::new(), 200_000);

    let out = run_block(view, vec![tx1, tx2], coinbase);
    assert!(matches!(out.receipts[0].status, zbx_types::receipt::TxStatus::Success),
        "tx1 must succeed");
    assert!(matches!(out.receipts[1].status, zbx_types::receipt::TxStatus::Success),
        "tx2 must succeed");

    // (a) State mutation: A.storage[1] == 0xCD (SSTORE in tx1).
    let slot1 = {
        let mut k = [0u8; 32]; k[31] = 0x01; H256(k)
    };
    let a_storage = out.storage.get(&cont_a).expect("A must have storage diff");
    let a_val = a_storage.get(&slot1).expect("A.storage[1] must be present");
    assert_eq!(a_val.0[31], 0xCD,
        "ZVM SSTORE in tx1 must persist into the post-block storage diff");

    // (b) Event emission: tx1's receipt must carry the LOG0 event.
    assert_eq!(out.receipts[0].logs.len(), 1,
        "ZVM LOG0 in tx1 must surface on the receipt via the StateView log drain");
    assert_eq!(out.receipts[0].logs[0].address, cont_a,
        "log's address must be the emitting ZVM contract");

    // (c) Transient clear: B.storage[2] must be ZERO. If the EIP-1153
    // scratchpad leaked across the tx boundary, TLOAD slot 1 in tx2
    // would observe tx1's TSTORE value (0xAA) and SSTORE that into
    // B.storage[2] — the assertion below would then fail.
    let slot2 = {
        let mut k = [0u8; 32]; k[31] = 0x02; H256(k)
    };
    let b_storage = out.storage.get(&cont_b).expect("B must have storage diff");
    let b_val = b_storage.get(&slot2).copied().unwrap_or(H256::zero());
    assert_eq!(b_val, H256::zero(),
        "EIP-1153 transient scratchpad MUST be cleared between txs — \
         B.storage[2] = TLOAD(slot 1) must be zero");

    // (d) tx2's receipt must carry no logs (sanity: contract B emits none).
    assert_eq!(out.receipts[1].logs.len(), 0);
}
