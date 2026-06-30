//! Transaction ordering for block building: EIP-1559 priority queue.

use zbx_types::{address::Address, U256};
use std::cmp::Ordering;
use std::collections::BinaryHeap;

/// A transaction's ordering metadata in the mempool.
#[derive(Debug, Clone)]
pub struct OrderedTx {
    pub sender:       Address,
    pub nonce:        u64,
    pub gas_limit:    u64,
    pub max_fee:      U256,
    pub max_priority: U256,
    pub effective_tip: U256, // miner_tip at current base_fee
    pub tx_hash:      [u8; 32],
}

impl PartialEq for OrderedTx {
    fn eq(&self, other: &Self) -> bool {
        self.effective_tip == other.effective_tip
            && self.gas_limit == other.gas_limit
            && self.sender == other.sender
            && self.nonce == other.nonce
    }
}
impl Eq for OrderedTx {}

impl PartialOrd for OrderedTx {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OrderedTx {
    fn cmp(&self, other: &Self) -> Ordering {
        // Higher effective tip → higher priority.
        // Tie-break: lower gas_limit (smaller txs first to pack more).
        // Tie-break: lower nonce (ordering within sender sequence).
        self.effective_tip.cmp(&other.effective_tip)
            .then(other.gas_limit.cmp(&self.gas_limit))
            .then(other.nonce.cmp(&self.nonce))
    }
}

/// A max-heap of transactions ordered by miner tip.
pub struct TxPriorityQueue {
    heap:     BinaryHeap<OrderedTx>,
    base_fee: U256,
}

impl TxPriorityQueue {
    pub fn new(base_fee: U256) -> Self {
        Self { heap: BinaryHeap::new(), base_fee }
    }

    pub fn push(&mut self, mut tx: OrderedTx) {
        // Recompute tip at current base_fee.
        tx.effective_tip = crate::pricing::miner_tip(tx.max_fee, tx.max_priority, self.base_fee);
        self.heap.push(tx);
    }

    pub fn pop(&mut self) -> Option<OrderedTx> { self.heap.pop() }

    pub fn peek(&self) -> Option<&OrderedTx> { self.heap.peek() }

    pub fn len(&self) -> usize { self.heap.len() }
    pub fn is_empty(&self) -> bool { self.heap.is_empty() }

    /// Update base fee and rebuild the heap.
    pub fn update_base_fee(&mut self, new_base: U256) {
        self.base_fee = new_base;
        let items: Vec<_> = self.heap.drain().collect();
        for mut tx in items {
            tx.effective_tip = crate::pricing::miner_tip(tx.max_fee, tx.max_priority, new_base);
            self.heap.push(tx);
        }
    }

    /// Drain up to `max_gas` worth of transactions for block building.
    pub fn drain_for_block(&mut self, max_gas: u64) -> Vec<OrderedTx> {
        let mut txs = Vec::new();
        let mut gas_used = 0u64;
        while let Some(tx) = self.heap.peek() {
            let next_gas = tx.gas_limit;
            if gas_used + next_gas > max_gas { break; }
            let tx = self.heap.pop().unwrap();
            gas_used += tx.gas_limit;
            txs.push(tx);
        }
        txs
    }
}