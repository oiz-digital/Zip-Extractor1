//! End-to-end integration test for the on-chain staking-tx pipeline.

use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;

use zbx_crypto::bls::BlsPrivKey;
use zbx_staking::{
    dispatch_staking_tx, decode_staking_call, BalanceAccess, StakingDelta, StakingError,
    validator::{Validator, ValidatorSet, ValidatorStatus}, MIN_SELF_STAKE,
};
use zbx_storage::ZbxDb;
use zbx_types::address::Address;
use zbx_types::staking_tx::{StakingTx, STAKING_PRECOMPILE_ADDR, UNBONDING_PERIOD_BLOCKS};

#[derive(Default)]
struct FakeBalances(HashMap<Address, u128>);
impl BalanceAccess for FakeBalances {
    fn get_balance(&self, a: &Address) -> u128 { *self.0.get(a).unwrap_or(&0) }
    fn set_balance(&mut self, a: &Address, w: u128) { self.0.insert(*a, w); }
}

fn fresh_db() -> (TempDir, Arc<ZbxDb>) {
    let tmp = TempDir::new().unwrap();
    let db = Arc::new(ZbxDb::open(tmp.path()).unwrap());
    (tmp, db)
}

fn make_pop(seed: u8, addr: &Address) -> ([u8; 48], [u8; 96]) {
    let sk = BlsPrivKey::from_bytes(&[seed; 32]).unwrap();
    let pk = sk.to_pubkey();
    let mut preimg = Vec::with_capacity(20 + 14);
    preimg.extend_from_slice(addr.as_bytes());
    preimg.extend_from_slice(b"zbx-bls-pop-v1");
    let msg = zbx_crypto::keccak::keccak256(&preimg);
    let pop = sk.sign(&msg);
    (*pk.as_bytes(), *pop.as_bytes())
}

fn seed_genesis_validator(vs: &mut ValidatorSet, addr: Address, seed: u8) {
    let sk = BlsPrivKey::from_bytes(&[seed; 32]).unwrap();
    vs.validators.insert(addr, Validator {
        address: addr, bls_pubkey: sk.to_pubkey(),
        self_stake: MIN_SELF_STAKE, delegated_stake: 0, commission_bps: 500,
        status: ValidatorStatus::Active, last_signed_block: 0,
        pending_rewards: 0, delegator_reward_pool: 0, pool_denominator: 0, registered_epoch: 0,
    });
}

#[test]
fn full_pipeline_register_delegate_undelegate_withdraw_claim() {
    let (_tmp, db) = fresh_db();
    let mut vs = ValidatorSet::new();
    let mut bal = FakeBalances::default();
    let mut delta = StakingDelta::new();

    // Register-with-PoP
    let v1 = Address([0xa1; 20]);
    let (pk1, pop1) = make_pop(11, &v1);
    let raw = StakingTx::RegisterValidator {
        pubkey: [0u8; 33], bls_pubkey: pk1, bls_pop: pop1,
        self_stake: MIN_SELF_STAKE, commission_bps: 750,
    }.encode().unwrap();
    let call = decode_staking_call(&raw).unwrap();
    dispatch_staking_tx(&call, v1, MIN_SELF_STAKE, 1, &mut vs, &db, &mut delta, &mut bal).unwrap();
    let v = vs.get(&v1).unwrap();
    assert_eq!(v.self_stake, MIN_SELF_STAKE);
    assert_eq!(v.commission_bps, 750);

    // Bad-PoP rejection.
    let v2 = Address([0xa2; 20]);
    let (pk2, _) = make_pop(22, &v2);
    let (_, wrong_pop) = make_pop(22, &Address([0xa3; 20]));
    let raw_bad = StakingTx::RegisterValidator {
        pubkey: [0u8; 33], bls_pubkey: pk2, bls_pop: wrong_pop,
        self_stake: MIN_SELF_STAKE, commission_bps: 100,
    }.encode().unwrap();
    let err = dispatch_staking_tx(
        &decode_staking_call(&raw_bad).unwrap(),
        v2, MIN_SELF_STAKE, 2, &mut vs, &db, &mut delta, &mut bal,
    ).unwrap_err();
    assert!(matches!(err, StakingError::InvalidEvidence(_)));
    assert!(vs.get(&v2).is_none());

    // Delegate
    let val_b = Address([0xb0; 20]);
    seed_genesis_validator(&mut vs, val_b, 99);

    let delegator = Address([0xc1; 20]);
    let amount = 25 * 10u128.pow(18);
    let raw = StakingTx::Delegate { validator: val_b, amount }.encode().unwrap();
    dispatch_staking_tx(&decode_staking_call(&raw).unwrap(),
                        delegator, amount, 100, &mut vs, &db, &mut delta, &mut bal).unwrap();
    {
        let cur = bal.get_balance(&STAKING_PRECOMPILE_ADDR);
        bal.set_balance(&STAKING_PRECOMPILE_ADDR, cur + amount);
    }
    assert_eq!(vs.get(&val_b).unwrap().delegated_stake, amount);
    assert_eq!(delta.get_delegation(&db, &val_b, &delegator).unwrap(), amount);

    // Undelegate
    let undelegated = 10 * 10u128.pow(18);
    let h_undel = 200u64;
    let raw = StakingTx::Undelegate { validator: val_b, amount: undelegated }.encode().unwrap();
    dispatch_staking_tx(&decode_staking_call(&raw).unwrap(),
                        delegator, 0, h_undel, &mut vs, &db, &mut delta, &mut bal).unwrap();
    assert_eq!(vs.get(&val_b).unwrap().delegated_stake, amount - undelegated);
    assert_eq!(delta.get_delegation(&db, &val_b, &delegator).unwrap(), amount - undelegated);

    // Undelegate-too-much rejected.
    let err = dispatch_staking_tx(
        &decode_staking_call(
            &StakingTx::Undelegate { validator: val_b, amount: amount * 100 }
                .encode().unwrap(),
        ).unwrap(),
        delegator, 0, h_undel + 1, &mut vs, &db, &mut delta, &mut bal,
    ).unwrap_err();
    assert!(matches!(err, StakingError::InsufficientDelegation { .. }));

    // Withdraw before maturity
    let err = dispatch_staking_tx(
        &decode_staking_call(&StakingTx::Withdraw { validator: val_b }.encode().unwrap()).unwrap(),
        delegator, 0, h_undel + 100, &mut vs, &db, &mut delta, &mut bal,
    ).unwrap_err();
    assert!(matches!(err, StakingError::NothingToWithdraw));

    // Withdraw after maturity
    let unlock = h_undel + UNBONDING_PERIOD_BLOCKS;
    let prior_delegator = bal.get_balance(&delegator);
    let prior_escrow = bal.get_balance(&STAKING_PRECOMPILE_ADDR);
    dispatch_staking_tx(
        &decode_staking_call(&StakingTx::Withdraw { validator: val_b }.encode().unwrap()).unwrap(),
        delegator, 0, unlock + 1, &mut vs, &db, &mut delta, &mut bal,
    ).unwrap();
    assert_eq!(bal.get_balance(&delegator), prior_delegator + undelegated);
    assert_eq!(bal.get_balance(&STAKING_PRECOMPILE_ADDR), prior_escrow - undelegated);

    // ClaimRewards credits validator + zeroes pending
    vs.get_mut(&val_b).unwrap().pending_rewards = 3 * 10u128.pow(18);
    let prior_val_bal = bal.get_balance(&val_b);
    dispatch_staking_tx(
        &decode_staking_call(&StakingTx::ClaimRewards { validator: val_b }.encode().unwrap()).unwrap(),
        val_b, 0, unlock + 2, &mut vs, &db, &mut delta, &mut bal,
    ).unwrap();
    assert_eq!(bal.get_balance(&val_b), prior_val_bal + 3 * 10u128.pow(18));
    assert_eq!(vs.get(&val_b).unwrap().pending_rewards, 0);

    // Non-validator claim rejected.
    let stranger = Address([0xee; 20]);
    let err = dispatch_staking_tx(
        &decode_staking_call(&StakingTx::ClaimRewards { validator: stranger }.encode().unwrap()).unwrap(),
        stranger, 0, unlock + 3, &mut vs, &db, &mut delta, &mut bal,
    ).unwrap_err();
    assert!(matches!(err, StakingError::NotAValidator(_)));
}

#[test]
fn same_block_repeated_undelegations_accumulate() {
    let (_tmp, db) = fresh_db();
    let mut vs = ValidatorSet::new();
    let mut bal = FakeBalances::default();
    let mut delta = StakingDelta::new();
    let val = Address([0xb1; 20]);
    seed_genesis_validator(&mut vs, val, 17);
    let delegator = Address([0xc2; 20]);

    let amount = 30 * 10u128.pow(18);
    dispatch_staking_tx(
        &decode_staking_call(&StakingTx::Delegate { validator: val, amount }
            .encode().unwrap()).unwrap(),
        delegator, amount, 50, &mut vs, &db, &mut delta, &mut bal,
    ).unwrap();
    {
        let cur = bal.get_balance(&STAKING_PRECOMPILE_ADDR);
        bal.set_balance(&STAKING_PRECOMPILE_ADDR, cur + amount);
    }

    let h = 300u64;
    let a1 = 5 * 10u128.pow(18);
    let a2 = 7 * 10u128.pow(18);
    dispatch_staking_tx(
        &decode_staking_call(
            &StakingTx::Undelegate { validator: val, amount: a1 }.encode().unwrap()
        ).unwrap(),
        delegator, 0, h, &mut vs, &db, &mut delta, &mut bal,
    ).unwrap();
    dispatch_staking_tx(
        &decode_staking_call(
            &StakingTx::Undelegate { validator: val, amount: a2 }.encode().unwrap()
        ).unwrap(),
        delegator, 0, h, &mut vs, &db, &mut delta, &mut bal,
    ).unwrap();

    assert_eq!(vs.get(&val).unwrap().delegated_stake, amount - a1 - a2);
    assert_eq!(delta.get_delegation(&db, &val, &delegator).unwrap(), amount - a1 - a2);

    let unlock = h + UNBONDING_PERIOD_BLOCKS;
    assert_eq!(delta.get_unbonding_entry(&db, unlock, &delegator, &val).unwrap(), a1 + a2);

    let prior_d = bal.get_balance(&delegator);
    let prior_e = bal.get_balance(&STAKING_PRECOMPILE_ADDR);
    dispatch_staking_tx(
        &decode_staking_call(&StakingTx::Withdraw { validator: val }.encode().unwrap()).unwrap(),
        delegator, 0, unlock + 1, &mut vs, &db, &mut delta, &mut bal,
    ).unwrap();
    assert_eq!(bal.get_balance(&delegator), prior_d + a1 + a2);
    assert_eq!(bal.get_balance(&STAKING_PRECOMPILE_ADDR), prior_e - (a1 + a2));
}

#[test]
fn unbonding_period_lookup_window_is_21_days() {
    assert_eq!(UNBONDING_PERIOD_BLOCKS, 907_200);
}

#[test]
fn precompile_address_is_canonical_0x0888() {
    let bytes = STAKING_PRECOMPILE_ADDR.as_bytes();
    assert!(bytes[..18].iter().all(|b| *b == 0));
    assert_eq!(bytes[18], 0x08);
    assert_eq!(bytes[19], 0x88);
}
