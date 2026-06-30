//! Block explorer indexer — ingests blocks and transactions into a queryable store.

use std::collections::HashMap;

/// Minimal query interface the explorer HTTP layer uses.
pub trait ExplorerDB: Send + Sync {
    fn block_by_number(&self, number: u64) -> Option<ExplorerBlock>;
    fn block_by_hash(&self, hash: &str) -> Option<ExplorerBlock>;
    fn tx_by_hash(&self, hash: &str) -> Option<ExplorerTx>;
    fn latest_block_number(&self) -> u64;
}

/// Lightweight block summary stored by the indexer.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExplorerBlock {
    pub number: u64,
    pub hash: String,
    pub parent_hash: String,
    pub timestamp: u64,
    pub tx_count: u32,
    pub gas_used: u64,
    pub gas_limit: u64,
}

/// Lightweight transaction summary stored by the indexer.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExplorerTx {
    pub hash: String,
    pub block_number: u64,
    pub from: String,
    pub to: Option<String>,
    pub value: String,
    pub gas_used: u64,
    pub status: bool,
}

/// In-memory implementation used in tests and the dev server.
#[derive(Default)]
pub struct MemExplorerDB {
    blocks_by_number: HashMap<u64, ExplorerBlock>,
    blocks_by_hash: HashMap<String, ExplorerBlock>,
    txs: HashMap<String, ExplorerTx>,
    latest: u64,
}

impl MemExplorerDB {
    pub fn new() -> Self { Self::default() }

    pub fn ingest_block(&mut self, block: ExplorerBlock) {
        if block.number > self.latest { self.latest = block.number; }
        self.blocks_by_hash.insert(block.hash.clone(), block.clone());
        self.blocks_by_number.insert(block.number, block);
    }

    pub fn ingest_tx(&mut self, tx: ExplorerTx) {
        self.txs.insert(tx.hash.clone(), tx);
    }
}

impl ExplorerDB for MemExplorerDB {
    fn block_by_number(&self, number: u64) -> Option<ExplorerBlock> {
        self.blocks_by_number.get(&number).cloned()
    }
    fn block_by_hash(&self, hash: &str) -> Option<ExplorerBlock> {
        self.blocks_by_hash.get(hash).cloned()
    }
    fn tx_by_hash(&self, hash: &str) -> Option<ExplorerTx> {
        self.txs.get(hash).cloned()
    }
    fn latest_block_number(&self) -> u64 { self.latest }
}
