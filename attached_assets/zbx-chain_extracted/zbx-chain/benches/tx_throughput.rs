//! Benchmark: mempool insertion and selection throughput.
//!
//! Run with:
//!   cargo bench --bench tx_throughput

#![feature(test)]
extern crate test;

use test::Bencher;
use zbx_types::{SignedTransaction, Address, U256};
use zbx_mempool::TxPool;

fn make_tx(from: Address, nonce: u64, gas_price: u64) -> SignedTransaction {
    SignedTransaction {
        nonce,
        gas_limit: 21_000,
        gas_price: U256::from(gas_price),
        to: Some(Address::from([0xde; 20])),
        value: U256::from(1u64),
        from,
        ..Default::default()
    }
}

#[bench]
fn bench_pool_insert_10k(b: &mut Bencher) {
    let pool = TxPool::new(Default::default());

    b.iter(|| {
        for i in 0..10_000u64 {
            let addr = Address::from_low_u64_be(i % 100);
            let tx   = make_tx(addr, i / 100, 1_000_000_000 + i);
            let _    = pool.add_transaction(tx);
        }
    });
}

#[bench]
fn bench_select_256_txs(b: &mut Bencher) {
    let pool = TxPool::new(Default::default());

    // Pre-fill.
    for i in 0..10_000u64 {
        let addr = Address::from_low_u64_be(i % 200);
        let tx   = make_tx(addr, i / 200, 1_000_000_000 + i);
        let _    = pool.add_transaction(tx);
    }

    b.iter(|| {
        let _ = pool.select_transactions(15_000_000); // 15M gas block limit
    });
}

#[bench]
fn bench_pool_pending_count(b: &mut Bencher) {
    let pool = TxPool::new(Default::default());
    for i in 0..5_000u64 {
        let addr = Address::from_low_u64_be(i % 50);
        let tx   = make_tx(addr, i / 50, 2_000_000_000);
        let _    = pool.add_transaction(tx);
    }
    b.iter(|| {
        let _ = pool.pending_count();
    });
}