//! Mempool content API — txpool_content, txpool_status, txpool_inspect (geth-compat).

use std::collections::HashMap;
use crate::types::{Address, TxHash, U256};
use crate::mempool::{PendingPool, QueuedPool};
use crate::tx::Transaction;

/// TxPool content response (geth-compatible)
#[derive(Debug, Clone, serde::Serialize)]
pub struct TxPoolContent {
    pub pending: HashMap<Address, HashMap<u64, TxSummary>>,
    pub queued: HashMap<Address, HashMap<u64, TxSummary>>,
}

/// TxPool status response
#[derive(Debug, Clone, serde::Serialize)]
pub struct TxPoolStatus {
    pub pending: usize,
    pub queued: usize,
}

/// TxPool inspect response (human-readable summary)
#[derive(Debug, Clone, serde::Serialize)]
pub struct TxPoolInspect {
    pub pending: HashMap<Address, HashMap<u64, String>>,
    pub queued: HashMap<Address, HashMap<u64, String>>,
}

/// Transaction summary for content API
#[derive(Debug, Clone, serde::Serialize)]
pub struct TxSummary {
    pub hash: TxHash,
    pub nonce: u64,
    pub from: Address,
    pub to: Option<Address>,
    pub value: U256,
    pub gas: u64,
    pub gas_price: u64,
    pub max_fee: u64,
    pub max_priority_fee: u64,
    pub input: Vec<u8>,
    pub tx_type: u8,
}

impl TxSummary {
    pub fn from_tx(tx: &Transaction) -> Self {
        Self {
            hash: tx.hash(),
            nonce: tx.nonce(),
            from: tx.recover_sender().unwrap_or_default(),
            to: tx.to(),
            value: tx.value(),
            gas: tx.gas_limit(),
            gas_price: tx.max_fee_per_gas(),
            max_fee: tx.max_fee_per_gas(),
            max_priority_fee: tx.max_priority_fee_per_gas(),
            input: tx.data().to_vec(),
            tx_type: tx.tx_type() as u8,
        }
    }

    /// Human-readable inspect string
    pub fn inspect_string(&self) -> String {
        format!(
            "{:?}: {} wei + {} gas x {} gwei",
            self.to.map(|a| format!("{:?}", a)).unwrap_or("contract_creation".into()),
            self.value,
            self.gas,
            self.gas_price / 1_000_000_000,
        )
    }
}

/// Content API implementation
pub struct ContentApi {
    pub pending: Arc<RwLock<PendingPool>>,
    pub queued: Arc<RwLock<QueuedPool>>,
}

use std::sync::{Arc, RwLock};

impl ContentApi {
    pub fn new(pending: Arc<RwLock<PendingPool>>, queued: Arc<RwLock<QueuedPool>>) -> Self {
        Self { pending, queued }
    }

    /// Get full content (all txs)
    pub fn content(&self) -> TxPoolContent {
        let pending_pool = self.pending.read().unwrap();
        let queued_pool = self.queued.read().unwrap();
        let mut pending_map: HashMap<Address, HashMap<u64, TxSummary>> = HashMap::new();
        let mut queued_map: HashMap<Address, HashMap<u64, TxSummary>> = HashMap::new();

        for tx in pending_pool.all_transactions() {
            let sender = tx.recover_sender().unwrap_or_default();
            pending_map.entry(sender).or_default().insert(tx.nonce(), TxSummary::from_tx(&tx));
        }
        for tx in queued_pool.all_transactions() {
            let sender = tx.recover_sender().unwrap_or_default();
            queued_map.entry(sender).or_default().insert(tx.nonce(), TxSummary::from_tx(&tx));
        }
        TxPoolContent { pending: pending_map, queued: queued_map }
    }

    /// Get status (counts only)
    pub fn status(&self) -> TxPoolStatus {
        TxPoolStatus {
            pending: self.pending.read().unwrap().len(),
            queued: self.queued.read().unwrap().len(),
        }
    }

    /// Inspect — human-readable summaries
    pub fn inspect(&self) -> TxPoolInspect {
        let content = self.content();
        TxPoolInspect {
            pending: content.pending.into_iter().map(|(addr, txs)| {
                (addr, txs.into_iter().map(|(n, t)| (n, t.inspect_string())).collect())
            }).collect(),
            queued: content.queued.into_iter().map(|(addr, txs)| {
                (addr, txs.into_iter().map(|(n, t)| (n, t.inspect_string())).collect())
            }).collect(),
        }
    }

    /// Get transactions by address
    pub fn get_by_address(&self, address: Address) -> AddressTransactions {
        let pending: Vec<TxSummary> = self.pending.read().unwrap()
            .get_by_sender(address)
            .iter()
            .map(TxSummary::from_tx)
            .collect();
        let queued: Vec<TxSummary> = self.queued.read().unwrap()
            .get_by_sender(address)
            .iter()
            .map(TxSummary::from_tx)
            .collect();
        AddressTransactions { address, pending, queued }
    }
}

/// All transactions for an address
#[derive(Debug, Clone, serde::Serialize)]
pub struct AddressTransactions {
    pub address: Address,
    pub pending: Vec<TxSummary>,
    pub queued: Vec<TxSummary>,
}