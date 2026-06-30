//! Integration tests: HotStuff BFT consensus.

use zbx_consensus::{RoundManager, SafetyRules, BlockStore};
use zbx_types::{Block, Address, H256, U256};
use zbx_crypto::bls::BlsKeyPair;

fn make_validator(idx: u8) -> (Address, BlsKeyPair) {
    let addr = Address::from([idx; 20]);
    let key  = BlsKeyPair::generate(&mut rand::thread_rng());
    (addr, key)
}

#[test]
fn test_single_validator_commits_block() {
    let (addr, key) = make_validator(1);
    let validators  = vec![addr];

    let block_store = BlockStore::new();
    let safety      = SafetyRules::new(addr);
    let mut manager = RoundManager::new(validators, block_store, safety);

    let block = Block {
        number: 1,
        parent_hash: H256::zero(),
        ..Default::default()
    };

    let result = manager.propose_block(block.clone());
    assert!(result.is_ok(), "proposal failed: {:?}", result);

    let committed = manager.try_commit();
    assert!(committed.is_some(), "block should be committed");
    assert_eq!(committed.unwrap().number, 1);
}

#[test]
fn test_quorum_threshold_3_of_4() {
    // 4 validators, need 3 votes (>2/3 majority).
    let validators: Vec<_> = (1..=4).map(make_validator).collect();
    let addrs: Vec<_> = validators.iter().map(|(a, _)| *a).collect();

    let block_store = BlockStore::new();
    let safety      = SafetyRules::new(addrs[0]);
    let mut manager = RoundManager::new(addrs.clone(), block_store, safety);

    let block = Block { number: 1, parent_hash: H256::zero(), ..Default::default() };
    manager.propose_block(block.clone()).expect("propose");

    // 2 votes — not enough.
    for (addr, key) in validators.iter().take(2) {
        let vote = zbx_consensus::Vote::new(block.hash, *addr, key);
        manager.receive_vote(vote);
    }
    assert!(manager.try_commit().is_none(), "should not commit with only 2/4 votes");

    // 3rd vote — now we have quorum.
    let (addr, key) = &validators[2];
    let vote = zbx_consensus::Vote::new(block.hash, *addr, key);
    manager.receive_vote(vote);
    assert!(manager.try_commit().is_some(), "should commit with 3/4 votes");
}

#[test]
fn test_safety_rules_reject_equivocation() {
    let (addr, _key) = make_validator(1);
    let mut safety = SafetyRules::new(addr);

    let block_a = Block { number: 5, parent_hash: H256::zero(), ..Default::default() };
    let block_b = Block { number: 5, parent_hash: H256::from([1u8; 32]), ..Default::default() };

    assert!(safety.sign_proposal(&block_a).is_ok());
    // Signing a conflicting block at same round must fail.
    assert!(safety.sign_proposal(&block_b).is_err(), "safety rules should reject equivocation");
}