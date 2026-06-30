//! Benchmark: block execution throughput.
//!
//! Measures how many EVM transactions per second the execution engine can process
//! under various workloads (ERC-20 transfer, simple storage, no-op).
//!
//! Run with:
//!   cargo bench --bench block_execution

#![feature(test)]
extern crate test;

use test::Bencher;
use zbx_types::{Block, SignedTransaction, Address, U256};
use zbx_execution::Executor;
use zbx_state::StateDB;
use zbx_evm::{EvmConfig, EvmContext};

fn make_erc20_transfer_tx(from: Address, to: Address, nonce: u64) -> SignedTransaction {
    // ERC-20 transfer calldata: transfer(address,uint256)
    let selector = [0xa9, 0x05, 0x9c, 0xbb]; // keccak256("transfer(address,uint256)")[:4]
    let mut calldata = selector.to_vec();
    calldata.extend_from_slice(&[0u8; 12]); // pad address to 32 bytes
    calldata.extend_from_slice(to.as_bytes());
    calldata.extend_from_slice(&[0u8; 24]); // pad u256
    calldata.extend_from_slice(&100u64.to_be_bytes());

    SignedTransaction {
        nonce,
        gas_limit: 60_000,
        gas_price: U256::from(1_000_000_000u64), // 1 gwei
        to: Some(to),
        value: U256::zero(),
        data: calldata,
        from,
        ..Default::default()
    }
}

#[bench]
fn bench_erc20_transfer(b: &mut Bencher) {
    let mut state = StateDB::new_in_memory();
    let executor  = Executor::new(EvmConfig::mainnet());

    let alice = Address::from([0x01; 20]);
    let bob   = Address::from([0x02; 20]);
    // Pre-fund alice.
    state.set_balance(alice, U256::from(1_000_000_000_000_000_000u64));

    let mut nonce = 0u64;
    b.iter(|| {
        let tx = make_erc20_transfer_tx(alice, bob, nonce);
        let _ = executor.execute_transaction(&tx, &mut state);
        nonce += 1;
    });
}

#[bench]
fn bench_simple_transfer(b: &mut Bencher) {
    let mut state = StateDB::new_in_memory();
    let executor  = Executor::new(EvmConfig::mainnet());

    let alice = Address::from([0x01; 20]);
    let bob   = Address::from([0x02; 20]);
    state.set_balance(alice, U256::from(1_000_000_000_000_000_000u64));

    let mut nonce = 0u64;
    b.iter(|| {
        let tx = SignedTransaction {
            nonce,
            gas_limit: 21_000,
            gas_price: U256::from(1_000_000_000u64),
            to: Some(bob),
            value: U256::from(1u64),
            data: Vec::new(),
            from: alice,
            ..Default::default()
        };
        let _ = executor.execute_transaction(&tx, &mut state);
        nonce += 1;
    });
}

#[bench]
fn bench_full_block_1000_txs(b: &mut Bencher) {
    let state    = StateDB::new_in_memory();
    let executor = Executor::new(EvmConfig::mainnet());

    let alice = Address::from([0x01; 20]);
    let bob   = Address::from([0x02; 20]);

    b.iter(|| {
        let mut s = state.clone();
        s.set_balance(alice, U256::from(u64::MAX));
        let mut block = Block::default();
        for i in 0..1000u64 {
            block.transactions.push(SignedTransaction {
                nonce: i,
                gas_limit: 21_000,
                gas_price: U256::from(1_000_000_000u64),
                to: Some(bob),
                value: U256::from(1u64),
                from: alice,
                ..Default::default()
            });
        }
        executor.execute_block(&block, &mut s).unwrap()
    });
}