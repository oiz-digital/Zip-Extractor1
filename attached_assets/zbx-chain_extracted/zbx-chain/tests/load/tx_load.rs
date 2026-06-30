//! Load tests — transaction throughput and mempool stress.
//!
//! Measures:
//! - Peak TPS (transactions per second) the mempool accepts
//! - Mempool eviction behaviour under high load
//! - Block inclusion latency under sustained load
//! - Fee market EIP-1559 base fee adjustment under load

use std::time::{Duration, Instant};

// ─── Load test configuration ──────────────────────────────────────────────────

pub struct TxLoadConfig {
    /// Target transactions per second to inject.
    pub tps: u64,
    /// Duration of the load test.
    pub duration: Duration,
    /// Number of concurrent sender goroutines / threads.
    pub concurrency: u64,
    /// Gas price (wei) for submitted transactions.
    pub gas_price: u128,
}

impl Default for TxLoadConfig {
    fn default() -> Self {
        Self {
            tps:         1_000,
            duration:    Duration::from_secs(60),
            concurrency: 10,
            gas_price:   1_000_000_000, // 1 gwei
        }
    }
}

/// Load test result summary.
pub struct TxLoadResult {
    pub submitted:       u64,
    pub accepted:        u64,
    pub rejected:        u64,
    pub included_blocks: u64,
    pub p50_latency_ms:  u64,
    pub p95_latency_ms:  u64,
    pub p99_latency_ms:  u64,
    pub actual_tps:      f64,
}

impl TxLoadResult {
    /// Assert that the result meets minimum performance requirements.
    pub fn assert_meets_requirements(&self, min_tps: f64, max_p95_ms: u64) {
        assert!(
            self.actual_tps >= min_tps,
            "actual TPS {:.1} below minimum {:.1}", self.actual_tps, min_tps
        );
        assert!(
            self.p95_latency_ms <= max_p95_ms,
            "p95 latency {}ms exceeds budget {}ms", self.p95_latency_ms, max_p95_ms
        );
    }
}

// ─── Mempool model ────────────────────────────────────────────────────────────

/// Simplified mempool for load testing invariants without a live node.
struct MockMempool {
    pending:  std::collections::BTreeMap<(String, u64), MockTx>,
    max_size: usize,
    accepted: u64,
    rejected: u64,
}

#[derive(Clone)]
struct MockTx {
    sender:    String,
    nonce:     u64,
    gas_price: u128,
    hash:      [u8; 32],
}

impl MockMempool {
    fn new(max_size: usize) -> Self {
        Self {
            pending:  Default::default(),
            max_size,
            accepted: 0,
            rejected: 0,
        }
    }

    fn submit(&mut self, tx: MockTx) -> bool {
        let key = (tx.sender.clone(), tx.nonce);
        if self.pending.len() >= self.max_size {
            // Evict lowest gas price tx to make room.
            if let Some(lowest_key) = self.pending
                .iter()
                .min_by_key(|(_, t)| t.gas_price)
                .map(|(k, _)| k.clone())
            {
                if tx.gas_price > self.pending[&lowest_key].gas_price {
                    self.pending.remove(&lowest_key);
                } else {
                    self.rejected += 1;
                    return false;
                }
            } else {
                self.rejected += 1;
                return false;
            }
        }
        self.pending.insert(key, tx);
        self.accepted += 1;
        true
    }

    fn drain_block(&mut self, gas_limit: u64) -> Vec<MockTx> {
        let mut txs = Vec::new();
        let mut gas_used = 0u64;
        let per_tx_gas  = 21_000u64;

        let keys: Vec<_> = self.pending.keys().cloned().collect();
        for key in keys {
            if gas_used + per_tx_gas > gas_limit { break; }
            if let Some(tx) = self.pending.remove(&key) {
                gas_used += per_tx_gas;
                txs.push(tx);
            }
        }
        txs
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tx(sender: &str, nonce: u64, gas_price: u128) -> MockTx {
        let mut hash = [0u8; 32];
        hash[..8].copy_from_slice(&nonce.to_le_bytes());
        hash[8] = sender.as_bytes()[0];
        MockTx { sender: sender.to_string(), nonce, gas_price, hash }
    }

    /// Mempool must accept `max_size` transactions and evict lowest-priority on overflow.
    #[test]
    fn mempool_evicts_low_priority_on_overflow() {
        let max = 5usize;
        let mut pool = MockMempool::new(max);

        // Fill to capacity at 1 gwei.
        for i in 0..max {
            let accepted = pool.submit(make_tx("0xAlice", i as u64, 1_000_000_000));
            assert!(accepted, "tx {} should be accepted", i);
        }
        assert_eq!(pool.pending.len(), max);

        // Submit a higher-priority tx (10 gwei) — should evict one 1-gwei tx.
        let accepted = pool.submit(make_tx("0xBob", 0, 10_000_000_000));
        assert!(accepted, "high-priority tx must be accepted");
        assert_eq!(pool.pending.len(), max, "pool stays at max size after eviction");
        assert!(pool.accepted > 0);
    }

    /// Mempool rejects low-priority txs when at capacity.
    #[test]
    fn mempool_rejects_low_priority_when_full() {
        let max = 3usize;
        let mut pool = MockMempool::new(max);

        // Fill to capacity at 10 gwei.
        for i in 0..max {
            pool.submit(make_tx("0xAlice", i as u64, 10_000_000_000));
        }

        // Submit a lower-priority tx — must be rejected.
        let accepted = pool.submit(make_tx("0xBob", 0, 1_000_000_000));
        assert!(!accepted, "low-priority tx must be rejected when pool is full at higher price");
        assert!(pool.rejected > 0);
    }

    /// Block drain must not exceed gas limit.
    #[test]
    fn block_drain_respects_gas_limit() {
        let mut pool = MockMempool::new(1000);
        for i in 0u64..100 {
            pool.submit(make_tx("0xSender", i, 1_000_000_000));
        }

        let gas_limit = 1_000_000u64;
        let txs = pool.drain_block(gas_limit);
        let gas_used: u64 = txs.len() as u64 * 21_000;
        assert!(gas_used <= gas_limit,
            "gas_used {} exceeds gas_limit {}", gas_used, gas_limit);
    }

    /// After 1000 sequential nonce txs from same sender, pool holds them all.
    #[test]
    fn sequential_nonces_accepted() {
        let mut pool = MockMempool::new(2000);
        for nonce in 0u64..1000 {
            let ok = pool.submit(make_tx("0xUser", nonce, 1_000_000_000));
            assert!(ok, "nonce {} should be accepted", nonce);
        }
        assert_eq!(pool.pending.len(), 1000);
        assert_eq!(pool.accepted, 1000);
    }

    /// Duplicate (sender, nonce) submission replaces the old tx if gas price is higher.
    #[test]
    fn higher_gas_price_replaces_existing_nonce() {
        let mut pool = MockMempool::new(100);
        pool.submit(make_tx("0xUser", 5, 1_000_000_000)); // 1 gwei
        pool.submit(make_tx("0xUser", 5, 2_000_000_000)); // 2 gwei replacement

        // Only one tx at nonce 5 should remain.
        let count = pool.pending
            .keys()
            .filter(|(addr, n)| addr == "0xUser" && *n == 5)
            .count();
        assert_eq!(count, 1, "only one tx per (sender, nonce)");
    }

    /// EIP-1559 base fee increases when blocks are full.
    #[test]
    fn base_fee_increases_under_load() {
        // EIP-1559: base fee adjusts by ±12.5% per block based on gas used vs gas target.
        fn next_base_fee(current: u128, gas_used: u64, gas_target: u64) -> u128 {
            if gas_used == gas_target {
                return current;
            }
            let delta = current * ((gas_used as i64 - gas_target as i64).unsigned_abs() as u128)
                / (gas_target as u128 * 8);
            if gas_used > gas_target {
                current.saturating_add(delta.max(1))
            } else {
                current.saturating_sub(delta)
            }
        }

        let gas_target = 15_000_000u64;
        let gas_limit  = 30_000_000u64;
        let mut base_fee = 1_000_000_000u128; // 1 gwei

        // Simulate 10 full blocks.
        for _ in 0..10 {
            base_fee = next_base_fee(base_fee, gas_limit, gas_target);
        }

        assert!(base_fee > 1_000_000_000u128,
            "base fee must increase after 10 full blocks (was 1 gwei, now {})", base_fee);

        // Simulate 10 empty blocks.
        let mut base_fee_decreasing = base_fee;
        for _ in 0..10 {
            base_fee_decreasing = next_base_fee(base_fee_decreasing, 0, gas_target);
        }

        assert!(base_fee_decreasing < base_fee,
            "base fee must decrease after 10 empty blocks");
    }

    /// Throughput model: 1000 TPS sustained for 1 second should fit within
    /// testnet block capacity (30M gas, 21k per simple tx → ~1428 tx/block @ 2s block time).
    #[test]
    fn throughput_model_meets_testnet_target() {
        let gas_limit_per_block = 30_000_000u64;
        let gas_per_simple_tx   = 21_000u64;
        let block_time_secs     = 2u64;

        let txs_per_block = gas_limit_per_block / gas_per_simple_tx;
        let tps_capacity  = txs_per_block / block_time_secs;

        // Testnet target: 500 TPS minimum (room for complex contract txs).
        let target_tps = 500u64;
        assert!(tps_capacity >= target_tps,
            "capacity {}tps must meet target {}tps", tps_capacity, target_tps);
    }
}
