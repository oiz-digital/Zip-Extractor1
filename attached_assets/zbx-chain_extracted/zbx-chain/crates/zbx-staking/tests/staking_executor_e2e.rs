//! End-to-end test for the staking pipeline through
//! `BlockExecutor::execute_with_staking` against a real `Block`.

use std::sync::Arc;
use tempfile::TempDir;

use zbx_crypto::bls::BlsPrivKey;
use zbx_execution::{BlockExecutor, StateView};
use zbx_staking::validator::ValidatorSet;
use zbx_storage::ZbxDb;
use zbx_trie::trie::MemoryTrieDB;
use zbx_types::{
    account::AccountState,
    address::Address,
    block::{Block, BlockBody, BlockHeader},
    staking_tx::{StakingTx, STAKING_PRECOMPILE_ADDR, UNBONDING_PERIOD_BLOCKS},
    transaction::{SignedTransaction, Signature, Transaction},
    H256, U256,
};

const GAS_LIMIT: u64 = 5_000_000;
const PRICE: u64 = 1_000_000_000;

fn make_pop(seed: u8, addr: &Address) -> ([u8; 48], [u8; 96]) {
    let sk = BlsPrivKey::from_bytes(&[seed; 32]).unwrap();
    let pk = sk.to_pubkey();
    let mut preimg = Vec::with_capacity(34);
    preimg.extend_from_slice(addr.as_bytes());
    preimg.extend_from_slice(b"zbx-bls-pop-v1");
    let pop = sk.sign(&zbx_crypto::keccak::keccak256(&preimg));
    (*pk.as_bytes(), *pop.as_bytes())
}

fn dummy_sig() -> Signature {
    Signature { v: 0, r: H256([0u8; 32]), s: H256([0u8; 32]) }
}

fn build_tx(
    from: Address,
    nonce: u64,
    value_wei: u128,
    data: Vec<u8>,
) -> SignedTransaction {
    let mut value_be = [0u8; 32];
    let mut buf = [0u8; 16];
    buf.copy_from_slice(&value_wei.to_be_bytes());
    value_be[16..].copy_from_slice(&buf);
    let tx = Transaction {
        tx_type: zbx_types::transaction::TxType::DynamicFee,
        chain_id: zbx_types::CHAIN_ID_TESTNET,
        nonce,
        max_priority_fee_per_gas: 0,
        max_fee_per_gas: PRICE,
        gas_limit: GAS_LIMIT,
        to: Some(STAKING_PRECOMPILE_ADDR),
        value: U256::from_big_endian(&value_be),
        data,
        access_list: vec![],
    };
    let signing_hash = tx.signing_hash();
    let sig = dummy_sig();
    let sig_bytes = sig.to_bytes();
    let mut hbuf = Vec::with_capacity(32 + 65);
    hbuf.extend_from_slice(signing_hash.as_bytes());
    hbuf.extend_from_slice(&sig_bytes);
    use sha3::{Digest, Keccak256};
    let hash = H256::from_slice(&Keccak256::digest(&hbuf));
    SignedTransaction { tx, sig, from, hash }
}

#[test]
fn block_executor_routes_staking_txs_and_mutates_validator_set() {
    let tmp = TempDir::new().unwrap();
    let db = Arc::new(ZbxDb::open(tmp.path()).unwrap());
    let mut vs = ValidatorSet::new();

    let validator_addr = Address([0xa1; 20]);
    let delegator      = Address([0xb2; 20]);
    let coinbase       = Address([0x99; 20]);

    // Five staking txs in one block:
    //   #0  validator: RegisterValidator { value = MIN_SELF_STAKE }
    //   #1  delegator: Delegate { value = 50 ZBX }
    //   #2  delegator: Undelegate { 20 ZBX, value = 0 }
    //   #3  delegator: Withdraw                 (will revert: not matured)
    //   #4  validator: ClaimRewards
    let min_self = zbx_staking::MIN_SELF_STAKE;
    let dlg_amt: u128 = 50 * 10u128.pow(18);
    let und_amt: u128 = 20 * 10u128.pow(18);

    let (pk, pop) = make_pop(7, &validator_addr);
    let txs = vec![
        build_tx(
            validator_addr, 0, min_self,
            StakingTx::RegisterValidator {
                pubkey: [0u8; 33], bls_pubkey: pk, bls_pop: pop,
                self_stake: min_self, commission_bps: 500,
            }.encode().unwrap(),
        ),
        build_tx(
            delegator, 0, dlg_amt,
            StakingTx::Delegate { validator: validator_addr, amount: dlg_amt }
                .encode().unwrap(),
        ),
        build_tx(
            delegator, 1, 0,
            StakingTx::Undelegate { validator: validator_addr, amount: und_amt }
                .encode().unwrap(),
        ),
        build_tx(delegator, 2, 0,
                 StakingTx::Withdraw { validator: validator_addr }.encode().unwrap()),
        build_tx(validator_addr, 1, 0,
                 StakingTx::ClaimRewards { validator: validator_addr }.encode().unwrap()),
    ];

    let header = BlockHeader {
        parent_hash: H256([0u8; 32]),
        uncle_hash: H256([0u8; 32]),
        coinbase,
        state_root: H256([0u8; 32]),
        transactions_root: H256([0u8; 32]),
        receipts_root: H256([0u8; 32]),
        logs_bloom: [0u8; 256],
        difficulty: U256::zero(),
        number: 100,
        gas_limit: zbx_types::BLOCK_GAS_LIMIT,
        gas_used: 0,
        timestamp: 1_700_000_000,
        extra_data: b"zbx-test".to_vec(),
        mix_hash: H256([0u8; 32]),
        nonce: 0,
        base_fee_per_gas: 1,
        committee_signature: vec![],
        epoch: 0,
        epoch_seed: None,
    };
    let block = Block { header, body: BlockBody { transactions: txs, uncles: vec![] } };

    // Seed StateView with funded sender accounts.
    let mut view = StateView::new();
    let starter: u128 = 1_000_000 * 10u128.pow(18); // 1M ZBX each (covers MIN_SELF_STAKE + gas)
    let mut a1 = AccountState::default();
    a1.set_balance_u128(starter);
    view.seed_account(validator_addr, a1);
    let mut a2 = AccountState::default();
    a2.set_balance_u128(starter);
    view.seed_account(delegator, a2);
    view.seed_account(coinbase, AccountState::default());
    view.seed_account(STAKING_PRECOMPILE_ADDR, AccountState::default());

    let exec = BlockExecutor::execute_with_staking(
        &block,
        view,
        MemoryTrieDB::default(),
        &mut vs,
        &db,
        None,
    ).expect("execute_with_staking must succeed");

    // ── Assert ValidatorSet was mutated by tx #0 (RegisterValidator) ──
    let v = vs.get(&validator_addr).expect("validator must exist post-block");
    assert_eq!(v.self_stake, min_self,
        "RegisterValidator did not credit self_stake — execute_inner did NOT route via execute_staking_tx");
    assert_eq!(v.commission_bps, 500);
    assert_eq!(v.delegated_stake, dlg_amt - und_amt,
        "Delegate + Undelegate did not net-mutate aggregate delegated_stake");

    // ── Assert per-delegator ledger reflects delegate-then-undelegate ──
    let ledger = exec.staking_delta
        .get_delegation(&db, &validator_addr, &delegator).unwrap();
    assert_eq!(ledger, dlg_amt - und_amt);

    // ── Assert unbonding entry queued at unlock_height = 100 + period ──
    let unlock = 100 + UNBONDING_PERIOD_BLOCKS;
    let unbonded = exec.staking_delta
        .get_unbonding_entry(&db, unlock, &delegator, &validator_addr).unwrap();
    assert_eq!(unbonded, und_amt,
        "Undelegate did not write the unbonding row via execute_staking_tx path");

    // ── Assert escrow on STAKING_PRECOMPILE_ADDR holds (self_stake + dlg_amt) ──
    // Withdraw (#3) MUST have reverted because nothing matured at h=100,
    // and ClaimRewards (#4) had nothing to claim, so escrow is intact.
    let escrow = exec.state_diff.accounts
        .get(&STAKING_PRECOMPILE_ADDR)
        .map(|a| a.balance_u128())
        .unwrap_or(0);
    assert_eq!(escrow, min_self + dlg_amt,
        "STAKING_PRECOMPILE escrow should equal self_stake + delegation; got {escrow}");

    // ── Assert receipts: 5 receipts total, txs #0/#1/#2/#4 success,
    //    #3 (Withdraw before maturity) revert.
    assert_eq!(exec.receipts.len(), 5);
    assert_eq!(exec.receipts[0].status, zbx_types::receipt::TxStatus::Success);
    assert_eq!(exec.receipts[1].status, zbx_types::receipt::TxStatus::Success);
    assert_eq!(exec.receipts[2].status, zbx_types::receipt::TxStatus::Success);
    assert_eq!(exec.receipts[3].status, zbx_types::receipt::TxStatus::Failure,
        "Withdraw at height 100 (pre-maturity) must revert");
    assert_eq!(exec.receipts[4].status, zbx_types::receipt::TxStatus::Success);
}

#[test]
fn legacy_execute_with_db_does_not_route_staking_when_no_ctx() {
    // Sanity: existing callers of `execute_with_db` (None staking_ctx)
    // do NOT route to the staking handler — the staking-precompile tx
    // falls into the EVM/MockHost path, leaving ValidatorSet untouched.
    // This protects back-compat for tests + bootstrap callers that
    // never carry validator state.
    let tmp = TempDir::new().unwrap();
    let _db = Arc::new(ZbxDb::open(tmp.path()).unwrap());
    let vs = ValidatorSet::new();
    let v_addr = Address([0xa1; 20]);
    let coinbase = Address([0x99; 20]);
    let (pk, pop) = make_pop(7, &v_addr);
    let tx = build_tx(
        v_addr, 0, zbx_staking::MIN_SELF_STAKE,
        StakingTx::RegisterValidator {
            pubkey: [0u8; 33], bls_pubkey: pk, bls_pop: pop,
            self_stake: zbx_staking::MIN_SELF_STAKE, commission_bps: 500,
        }.encode().unwrap(),
    );
    let header = BlockHeader {
        parent_hash: H256([0u8; 32]), uncle_hash: H256([0u8; 32]),
        coinbase, state_root: H256([0u8; 32]),
        transactions_root: H256([0u8; 32]), receipts_root: H256([0u8; 32]),
        logs_bloom: [0u8; 256], difficulty: U256::zero(),
        number: 1, gas_limit: zbx_types::BLOCK_GAS_LIMIT, gas_used: 0,
        timestamp: 1_700_000_000, extra_data: b"x".to_vec(),
        mix_hash: H256([0u8; 32]), nonce: 0, base_fee_per_gas: 1,
        committee_signature: vec![], epoch: 0, epoch_seed: None,
    };
    let block = Block { header, body: BlockBody { transactions: vec![tx], uncles: vec![] } };
    let mut view = StateView::new();
    let mut acct = AccountState::default();
    acct.set_balance_u128(100_000 * 10u128.pow(18));
    view.seed_account(v_addr, acct);
    view.seed_account(coinbase, AccountState::default());

    let _ = BlockExecutor::execute_with_db(&block, view, MemoryTrieDB::default());

    // ValidatorSet must be empty — legacy path did NOT route.
    assert!(vs.get(&v_addr).is_none(),
        "execute_with_db (no staking_ctx) must NOT mutate ValidatorSet — back-compat broken");
}
