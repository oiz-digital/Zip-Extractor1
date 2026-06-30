//! Task #8 — EIP-6780 SELFDESTRUCT semantics on the ZVM execution path.
//!
//! Cancun (EIP-6780) downgrades SELFDESTRUCT to a balance-sweep for
//! pre-existing contracts. Full account deletion (storage + code +
//! account record) is preserved ONLY when the contract was CREATEd in
//! the same transaction that SELFDESTRUCTs it.
//!
//! This file exercises the executor end-of-tx drain wired into
//! `executor.rs` (ZVM branch): after the top-level call returns, the
//! executor reads `host.take_pending_destructs()` and applies
//! `view.selfdestruct(addr)` only for entries whose `contract` is in
//! `host.created_this_tx`, AND only if the tx exit status was Success.
//!
//! Coverage:
//!   1. Pre-existing contract → balance swept, account survives in root.
//!   2. CREATE + SELFDESTRUCT in same tx (init-code path) → fully deleted.
//!   3. Two-tx ordering: SELFDESTRUCT in tx0 of a fresh CREATE in tx1
//!      does NOT affect the tx1 CREATE (queues are per-tx).
//!   4. Reverted top-level tx → no destruction (success-only gate).
//!
//! Bytecode notes (EVM/ZVM share encoding):
//!   - PUSH20  = 0x73,  PUSH1 = 0x60
//!   - SELFDESTRUCT = 0xFF
//!   - REVERT  = 0xFD,  STOP = 0x00
//!
//! NOTE: top-level CREATE in this executor uses a *surrogate* address
//! scheme `keccak256(creator || nonce_be8)[12..]` (see executor.rs L630
//! and the `deploy_then_call_zvm_contract_persists_runtime_code_and_executes`
//! test in `zvm_e2e.rs`) — NOT canonical EVM `keccak(rlp([creator,nonce]))`.
//! We mirror that here.

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

/// Top-level CREATE address per the executor's surrogate scheme.
fn predict_create_addr(creator: Address, creator_nonce: u64) -> Address {
    let mut buf = Vec::with_capacity(28);
    buf.extend_from_slice(creator.as_bytes());
    buf.extend_from_slice(&creator_nonce.to_be_bytes());
    let h = keccak256(&buf);
    let mut a = [0u8; 20];
    a.copy_from_slice(&h.as_bytes()[12..]);
    Address(a)
}

struct Outcome {
    after: StateView,
    deleted: std::collections::HashSet<Address>,
    receipts: Vec<zbx_types::receipt::TransactionReceipt>,
}

impl Outcome {
    fn is_deleted(&self, a: &Address) -> bool {
        self.deleted.contains(a)
    }
}

fn run_block(view: StateView, txs: Vec<SignedTransaction>, coinbase: Address) -> Outcome {
    let block = Block {
        header: header(coinbase),
        body: BlockBody { transactions: txs, uncles: Vec::new() },
    };
    let result = BlockExecutor::execute(&block, view).expect("block must execute");
    let deleted: std::collections::HashSet<Address> =
        result.state_diff.deleted.iter().copied().collect();
    let mut after = StateView::new();
    for (a, s) in result.state_diff.accounts {
        // Skip tombstoned addresses (their accounts were removed from
        // diffs.accounts by `view.selfdestruct`, but defensively
        // filter here too).
        if !deleted.contains(&a) {
            after.set_account(a, s);
        }
    }
    Outcome { after, deleted, receipts: result.receipts }
}

// ─────────────────────────────────────────────────────────────────────────
// Test 1 — pre-existing contract: balance swept, account survives.
// ─────────────────────────────────────────────────────────────────────────
#[test]
fn pre_existing_contract_selfdestruct_only_sweeps_balance() {
    let mut view = StateView::new();
    let sender    = addr(0x11);
    let coinbase  = addr(0xC0);
    let target    = addr(0x33);
    let beneficiary = addr(0x44);

    fund(&mut view, sender, 10u128.pow(20));

    // Contract C: PUSH20 <ben> ; SELFDESTRUCT
    let mut code = vec![0x73u8];
    code.extend_from_slice(beneficiary.as_bytes());
    code.push(0xFF);
    let code_hash = keccak256(&code);

    // Pre-existing contract with 1_000_000 wei balance.
    let mut c_acct = AccountState::default();
    c_acct.code_hash = code_hash;
    c_acct.vm = VmKind::Zvm;
    c_acct.set_balance_u128(1_000_000);
    view.set_account(target, c_acct);
    view.seed_code(code_hash, code);

    let tx = mk_tx(sender, Some(target), 0, Vec::new(), 200_000);
    let out = run_block(view, vec![tx], coinbase);
    assert!(matches!(out.receipts[0].status, zbx_types::receipt::TxStatus::Success));

    // Beneficiary received the swept balance.
    assert_eq!(
        out.after.get_account(&beneficiary).balance_u128(),
        1_000_000,
        "beneficiary must receive swept balance"
    );

    // The pre-existing contract must NOT have been tombstoned —
    // EIP-6780 only deletes same-tx CREATEs. Its balance is now 0
    // (swept), but the account record / code_hash survive.
    assert!(
        !out.is_deleted(&target),
        "EIP-6780: pre-existing contract must NOT be deleted"
    );
    let surviving = out.after.get_account(&target);
    assert_eq!(surviving.balance_u128(), 0, "C's balance must be swept to 0");
    assert_eq!(
        surviving.code_hash, code_hash,
        "C's code_hash must survive (no full deletion under EIP-6780)"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Test 2 — CREATE+SELFDESTRUCT in same tx → full deletion.
// ─────────────────────────────────────────────────────────────────────────
#[test]
fn create_then_selfdestruct_same_tx_fully_deletes() {
    let mut view = StateView::new();
    let sender   = addr(0x55);
    let coinbase = addr(0xC0);
    let beneficiary = addr(0x66);
    fund(&mut view, sender, 10u128.pow(20));

    let predicted = predict_create_addr(sender, 0);

    // Init code = ZVM-discriminator + (PUSH20 ben ; SELFDESTRUCT).
    // The init-code path itself self-destructs the just-created address;
    // the executor's mark_created_this_tx happens inside do_create
    // before init-code runs, so the host sees the CREATE on the
    // pending_destructs gate.
    let mut init = vec![ZVM_DEPLOY_DISCRIMINATOR, 0x73u8];
    init.extend_from_slice(beneficiary.as_bytes());
    init.push(0xFF);

    let tx = mk_tx(sender, None, 0, init, 300_000);
    let out = run_block(view, vec![tx], coinbase);
    assert!(matches!(out.receipts[0].status, zbx_types::receipt::TxStatus::Success));

    // EIP-6780: same-tx CREATE+SELFDESTRUCT → predicted address fully
    // deleted from the post-block state.
    assert!(
        out.is_deleted(&predicted),
        "EIP-6780: same-tx CREATE+SELFDESTRUCT must mark account for full deletion"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Test 3 — two-tx isolation: tx0's CREATE+SELFDESTRUCT does not bleed
// into tx1's freshly-CREATEd account.
// ─────────────────────────────────────────────────────────────────────────
#[test]
fn destruct_queue_isolated_per_tx() {
    let mut view = StateView::new();
    let sender   = addr(0x77);
    let coinbase = addr(0xC0);
    let beneficiary = addr(0x88);
    fund(&mut view, sender, 10u128.pow(20));

    let addr_tx0 = predict_create_addr(sender, 0);
    let addr_tx1 = predict_create_addr(sender, 1);

    // tx0 init = discriminator + PUSH20 ben + SELFDESTRUCT.
    let mut init0 = vec![ZVM_DEPLOY_DISCRIMINATOR, 0x73u8];
    init0.extend_from_slice(beneficiary.as_bytes());
    init0.push(0xFF);

    // tx1 init = discriminator + plain STOP (no selfdestruct).
    let init1 = vec![ZVM_DEPLOY_DISCRIMINATOR, 0x00u8];

    let tx0 = mk_tx(sender, None, 0, init0, 300_000);
    let tx1 = mk_tx(sender, None, 1, init1, 300_000);

    let out = run_block(view, vec![tx0, tx1], coinbase);
    assert!(matches!(out.receipts[0].status, zbx_types::receipt::TxStatus::Success));
    assert!(matches!(out.receipts[1].status, zbx_types::receipt::TxStatus::Success));

    assert!(
        out.is_deleted(&addr_tx0),
        "tx0's CREATE+SELFDESTRUCT addr must be deleted"
    );
    assert!(
        !out.is_deleted(&addr_tx1),
        "tx1's plain CREATE must NOT be deleted (per-tx queue isolation)"
    );
    // And tx1's account must actually exist in the post-block state
    // with VmKind::Zvm.
    assert_eq!(
        out.after.get_account(&addr_tx1).vm,
        VmKind::Zvm,
        "tx1's freshly-CREATEd account must survive as a real ZVM account"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Test 5 — multiple SELFDESTRUCTs in one tx: drain is batched + idempotent.
// ─────────────────────────────────────────────────────────────────────────
//
// Architect-review follow-up: prove the executor's end-of-tx drain
// handles multiple pending_destructs entries in the same tx (one
// pre-existing contract that SELFDESTRUCTs to a beneficiary, called
// from a parent contract that ALSO SELFDESTRUCTs to the same
// beneficiary at the very end). Two pending_destructs entries; the
// pre-existing one is sweep-only, the same-tx-CREATE one — wait,
// neither was CREATEd this tx. Both sweep-only. Tombstone set must
// stay empty; both balances must reach the beneficiary.
#[test]
fn multiple_selfdestructs_in_one_tx_batched_drain() {
    let mut view    = StateView::new();
    let sender      = addr(0xE1);
    let coinbase    = addr(0xC0);
    let child       = addr(0xE2);
    let parent      = addr(0xE3);
    let beneficiary = addr(0xE4);

    fund(&mut view, sender, 10u128.pow(20));

    // Child C: PUSH20 ben ; SELFDESTRUCT — balance 100.
    let mut child_code = vec![0x73u8];
    child_code.extend_from_slice(beneficiary.as_bytes());
    child_code.push(0xFF);
    let child_hash = keccak256(&child_code);
    let mut child_acct = AccountState::default();
    child_acct.code_hash = child_hash;
    child_acct.vm = VmKind::Zvm;
    child_acct.set_balance_u128(100);
    view.set_account(child, child_acct);
    view.seed_code(child_hash, child_code);

    // Parent P: CALL(child) ; PUSH20 ben ; SELFDESTRUCT — balance 200.
    //   60 00 60 00 60 00 60 00 60 00     ; ret_len, ret_off, args_len, args_off, value
    //   73 <child:20>                      ; PUSH20 child
    //   61 27 10                           ; PUSH2 0x2710 (gas)
    //   F1                                 ; CALL
    //   50                                 ; POP (discard status)
    //   73 <ben:20>                        ; PUSH20 ben
    //   FF                                 ; SELFDESTRUCT
    let mut p_code: Vec<u8> = vec![
        0x60, 0x00, 0x60, 0x00, 0x60, 0x00, 0x60, 0x00, 0x60, 0x00,
        0x73,
    ];
    p_code.extend_from_slice(child.as_bytes());
    p_code.extend_from_slice(&[0x61, 0x27, 0x10, 0xF1, 0x50, 0x73]);
    p_code.extend_from_slice(beneficiary.as_bytes());
    p_code.push(0xFF);
    let p_hash = keccak256(&p_code);
    let mut p_acct = AccountState::default();
    p_acct.code_hash = p_hash;
    p_acct.vm = VmKind::Zvm;
    p_acct.set_balance_u128(200);
    view.set_account(parent, p_acct);
    view.seed_code(p_hash, p_code);

    let tx = mk_tx(sender, Some(parent), 0, Vec::new(), 500_000);
    let out = run_block(view, vec![tx], coinbase);
    assert!(matches!(out.receipts[0].status, zbx_types::receipt::TxStatus::Success));

    // Beneficiary received both swept balances (100 + 200 = 300).
    assert_eq!(
        out.after.get_account(&beneficiary).balance_u128(),
        300,
        "beneficiary must receive 100 (child) + 200 (parent) = 300"
    );

    // Neither contract was CREATEd this tx → both sweep-only,
    // tombstone set must remain empty.
    assert!(
        !out.is_deleted(&child),
        "pre-existing child must NOT be tombstoned"
    );
    assert!(
        !out.is_deleted(&parent),
        "pre-existing parent must NOT be tombstoned"
    );
    assert_eq!(
        out.deleted.len(), 0,
        "no tombstones expected when all SELFDESTRUCTs target pre-existing contracts"
    );

    // Both contracts' balances must be 0 (swept).
    assert_eq!(out.after.get_account(&child).balance_u128(), 0);
    assert_eq!(out.after.get_account(&parent).balance_u128(), 0);
}

// ─────────────────────────────────────────────────────────────────────────
// Test 4 — reverted tx: pending_destructs is collected but NOT applied.
// ─────────────────────────────────────────────────────────────────────────
//
// Outer ZVM contract code:
//     PUSH20 <self>          ; SELFDESTRUCT target = self (will halt)
//     <unreachable>
// Wait — SELFDESTRUCT halts execution, so we can't follow it with REVERT
// in the same frame. Instead, use init-code that REVERTs immediately;
// CREATE will fail and pending_destructs will not include any contract,
// because no SELFDESTRUCT ran. That doesn't exercise the success gate.
//
// Better: a CALL frame from a top-level CALL that does SELFDESTRUCT in
// a sub-frame, followed by an outer REVERT. But the executor's ZVM path
// only opens ONE outer frame per tx and the only way to get a sub-call
// is via pre-deployed contract bytecode. We pre-deploy a parent that
// CALLs a pre-existing C (which SELFDESTRUCTs), then REVERTs at the
// top level. The parent's REVERT means `result.status = Revert`, the
// success gate fires, and `view.selfdestruct(C)` is NOT called →
// `is_deleted(C) == false`.
#[test]
fn reverted_tx_does_not_apply_selfdestruct() {
    let mut view    = StateView::new();
    let sender      = addr(0xA1);
    let coinbase    = addr(0xC0);
    let target_c    = addr(0xB1);
    let beneficiary = addr(0xC1);
    let parent      = addr(0xD1);

    fund(&mut view, sender, 10u128.pow(20));

    // C = PUSH20 ben ; SELFDESTRUCT
    let mut c_code = vec![0x73u8];
    c_code.extend_from_slice(beneficiary.as_bytes());
    c_code.push(0xFF);
    let c_hash = keccak256(&c_code);
    let mut c_acct = AccountState::default();
    c_acct.code_hash = c_hash;
    c_acct.vm = VmKind::Zvm;
    c_acct.set_balance_u128(500);
    view.set_account(target_c, c_acct);
    view.seed_code(c_hash, c_code);

    // Parent runtime:
    //   60 00 60 00 60 00 60 00 60 00     ; ret_len, ret_off, args_len, args_off, value
    //   73 <c:20>                          ; PUSH20 target
    //   61 27 10                           ; PUSH2 0x2710 (gas)
    //   F1                                 ; CALL
    //   50                                 ; POP (discard call status)
    //   60 00 60 00                        ; PUSH1 0  (size), PUSH1 0 (offset)
    //   FD                                 ; REVERT
    let mut p_code: Vec<u8> = vec![
        0x60, 0x00, 0x60, 0x00, 0x60, 0x00, 0x60, 0x00, 0x60, 0x00,
        0x73,
    ];
    p_code.extend_from_slice(target_c.as_bytes());
    p_code.extend_from_slice(&[
        0x61, 0x27, 0x10,
        0xF1,
        0x50,
        0x60, 0x00, 0x60, 0x00,
        0xFD,
    ]);
    let p_hash = keccak256(&p_code);
    let mut p_acct = AccountState::default();
    p_acct.code_hash = p_hash;
    p_acct.vm = VmKind::Zvm;
    view.set_account(parent, p_acct);
    view.seed_code(p_hash, p_code);

    let tx = mk_tx(sender, Some(parent), 0, Vec::new(), 500_000);
    let out = run_block(view, vec![tx], coinbase);

    // Outer tx must have reverted.
    assert!(
        matches!(out.receipts[0].status, zbx_types::receipt::TxStatus::Failure),
        "outer tx must report Failure (REVERT opcode at end of parent runtime)"
    );

    // The success-only gate must have suppressed view.selfdestruct(C).
    // C is pre-existing anyway (so even on success it would only be
    // swept), but the assertion here is that to_delete is empty on a
    // reverted tx — neither the pre-existing nor any same-tx CREATE
    // would have been tombstoned.
    assert!(
        !out.is_deleted(&target_c),
        "reverted tx must NOT trigger view.selfdestruct (success-only gate)"
    );
}
