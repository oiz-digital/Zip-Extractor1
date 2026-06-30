//! Block indexer — indexes every block into a queryable store.
//! Supports block-by-hash, block-by-number, transactions, receipts, logs.

use std::sync::Arc;
use std::collections::HashMap;
use rocksdb::{DB, ColumnFamily, Options, WriteBatch};
use crate::types::{BlockHash, Address, U256, TxHash};

/// Column family names
pub const CF_BLOCKS: &str = "blocks";
pub const CF_BLOCK_NUM: &str = "block_num";
pub const CF_TX: &str = "transactions";
pub const CF_RECEIPTS: &str = "receipts";
pub const CF_LOGS: &str = "logs";
pub const CF_ADDR_TX: &str = "addr_tx";
pub const CF_METADATA: &str = "metadata";

/// Block index entry
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BlockIndex {
    pub number: u64,
    pub hash: BlockHash,
    pub parent_hash: BlockHash,
    pub timestamp: u64,
    pub tx_count: u32,
    pub gas_used: u64,
    pub gas_limit: u64,
    pub base_fee: U256,
    pub miner: Address,
    pub size: u64,
    pub state_root: [u8; 32],
    pub receipts_root: [u8; 32],
    pub tx_hashes: Vec<TxHash>,
}

/// Transaction index entry
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TxIndex {
    pub hash: TxHash,
    pub block_hash: BlockHash,
    pub block_number: u64,
    pub tx_index: u32,
    pub from: Address,
    pub to: Option<Address>,
    pub value: U256,
    pub gas_price: u64,
    pub gas: u64,
    pub nonce: u64,
    pub input_len: usize,
    pub status: bool,
    pub gas_used: u64,
    pub contract_address: Option<Address>,
}

/// Block indexer
pub struct BlockIndexer {
    pub db: Arc<DB>,
    pub latest_indexed: u64,
    pub total_indexed: u64,
    pub config: IndexerConfig,
}

#[derive(Debug, Clone)]
pub struct IndexerConfig {
    pub db_path: String,
    pub batch_size: usize,
    pub enable_log_index: bool,
    pub enable_addr_index: bool,
    pub max_reorg_depth: u64,
}

impl Default for IndexerConfig {
    fn default() -> Self {
        Self {
            db_path: "./data/indexer".into(),
            batch_size: 1000,
            enable_log_index: true,
            enable_addr_index: true,
            max_reorg_depth: 128,
        }
    }
}

impl BlockIndexer {
    pub fn new(config: IndexerConfig) -> Result<Self, IndexerError> {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);
        let cfs = vec![CF_BLOCKS, CF_BLOCK_NUM, CF_TX, CF_RECEIPTS, CF_LOGS, CF_ADDR_TX, CF_METADATA];
        let db = DB::open_cf(&opts, &config.db_path, &cfs)
            .map_err(|e| IndexerError::Database(e.to_string()))?;
        Ok(Self { db: Arc::new(db), latest_indexed: 0, total_indexed: 0, config })
    }

    /// Index a single block
    pub fn index_block(&mut self, block: &BlockIndex, txs: &[TxIndex]) -> Result<(), IndexerError> {
        let mut batch = WriteBatch::default();
        // Store block by hash
        let block_bytes = bincode::serialize(block).map_err(|e| IndexerError::Serialization(e.to_string()))?;
        self.put_cf_batch(&mut batch, CF_BLOCKS, &block.hash.0, &block_bytes);
        // Store block number -> hash mapping
        self.put_cf_batch(&mut batch, CF_BLOCK_NUM, &block.number.to_be_bytes(), &block.hash.0);
        // Store each transaction
        for tx in txs {
            let tx_bytes = bincode::serialize(tx).map_err(|e| IndexerError::Serialization(e.to_string()))?;
            self.put_cf_batch(&mut batch, CF_TX, &tx.hash.0, &tx_bytes);
            // Address -> tx mapping
            if self.config.enable_addr_index {
                let from_key = Self::addr_tx_key(&tx.from, block.number, tx.tx_index);
                self.put_cf_batch(&mut batch, CF_ADDR_TX, &from_key, &tx.hash.0);
                if let Some(to) = &tx.to {
                    let to_key = Self::addr_tx_key(to, block.number, tx.tx_index);
                    self.put_cf_batch(&mut batch, CF_ADDR_TX, &to_key, &tx.hash.0);
                }
            }
        }
        // Update metadata (latest block)
        self.put_cf_batch(&mut batch, CF_METADATA, b"latest_block", &block.number.to_be_bytes());
        self.db.write(batch).map_err(|e| IndexerError::Database(e.to_string()))?;
        self.latest_indexed = block.number;
        self.total_indexed += 1;
        tracing::debug!(block = block.number, txs = txs.len(), "Block indexed");
        Ok(())
    }

    /// Get block by hash
    pub fn get_block(&self, hash: &BlockHash) -> Result<Option<BlockIndex>, IndexerError> {
        let cf = self.db.cf_handle(CF_BLOCKS).ok_or(IndexerError::CfNotFound(CF_BLOCKS))?;
        match self.db.get_cf(cf, &hash.0) {
            Ok(Some(bytes)) => Ok(Some(bincode::deserialize(&bytes).map_err(|e| IndexerError::Serialization(e.to_string()))?)),
            Ok(None) => Ok(None),
            Err(e) => Err(IndexerError::Database(e.to_string())),
        }
    }

    /// Get block by number
    pub fn get_block_by_number(&self, number: u64) -> Result<Option<BlockIndex>, IndexerError> {
        let cf = self.db.cf_handle(CF_BLOCK_NUM).ok_or(IndexerError::CfNotFound(CF_BLOCK_NUM))?;
        let hash_bytes = match self.db.get_cf(cf, &number.to_be_bytes()) {
            Ok(Some(b)) => b,
            Ok(None) => return Ok(None),
            Err(e) => return Err(IndexerError::Database(e.to_string())),
        };
        let mut hash = BlockHash([0u8; 32]);
        hash.0.copy_from_slice(&hash_bytes[..32.min(hash_bytes.len())]);
        self.get_block(&hash)
    }

    /// Get transaction by hash
    pub fn get_tx(&self, hash: &TxHash) -> Result<Option<TxIndex>, IndexerError> {
        let cf = self.db.cf_handle(CF_TX).ok_or(IndexerError::CfNotFound(CF_TX))?;
        match self.db.get_cf(cf, &hash.0) {
            Ok(Some(bytes)) => Ok(Some(bincode::deserialize(&bytes).map_err(|e| IndexerError::Serialization(e.to_string()))?)),
            Ok(None) => Ok(None),
            Err(e) => Err(IndexerError::Database(e.to_string())),
        }
    }

    /// Get all txs from/to address
    pub fn get_txs_by_address(&self, addr: &Address, limit: usize) -> Result<Vec<TxIndex>, IndexerError> {
        let cf_addr = self.db.cf_handle(CF_ADDR_TX).ok_or(IndexerError::CfNotFound(CF_ADDR_TX))?;
        let prefix = addr.0.to_vec();
        let iter = self.db.prefix_iterator_cf(cf_addr, &prefix);
        let mut results = Vec::new();
        for item in iter.take(limit) {
            let (_, tx_hash_bytes) = item.map_err(|e| IndexerError::Database(e.to_string()))?;
            let mut tx_hash = TxHash([0u8; 32]);
            tx_hash.0.copy_from_slice(&tx_hash_bytes[..32.min(tx_hash_bytes.len())]);
            if let Some(tx) = self.get_tx(&tx_hash)? {
                results.push(tx);
            }
        }
        Ok(results)
    }

    /// Handle chain reorg: delete blocks above fork_point
    pub fn handle_reorg(&mut self, fork_point: u64, fork_hash: &BlockHash) -> Result<u64, IndexerError> {
        let reorg_depth = self.latest_indexed.saturating_sub(fork_point);
        if reorg_depth > self.config.max_reorg_depth {
            return Err(IndexerError::ReorgTooDeep { depth: reorg_depth, max: self.config.max_reorg_depth });
        }
        let mut removed = 0u64;
        for num in (fork_point + 1..=self.latest_indexed).rev() {
            if let Some(block) = self.get_block_by_number(num)? {
                self.remove_block(&block)?;
                removed += 1;
            }
        }
        self.latest_indexed = fork_point;
        tracing::warn!(fork_point, reorg_depth, removed, "Chain reorg processed");
        Ok(removed)
    }

    fn remove_block(&self, block: &BlockIndex) -> Result<(), IndexerError> {
        let mut batch = WriteBatch::default();
        let cf_blocks = self.db.cf_handle(CF_BLOCKS).ok_or(IndexerError::CfNotFound(CF_BLOCKS))?;
        batch.delete_cf(cf_blocks, &block.hash.0);
        let cf_num = self.db.cf_handle(CF_BLOCK_NUM).ok_or(IndexerError::CfNotFound(CF_BLOCK_NUM))?;
        batch.delete_cf(cf_num, &block.number.to_be_bytes());
        self.db.write(batch).map_err(|e| IndexerError::Database(e.to_string()))
    }

    fn put_cf_batch(&self, batch: &mut WriteBatch, cf_name: &str, key: &[u8], value: &[u8]) {
        if let Some(cf) = self.db.cf_handle(cf_name) {
            batch.put_cf(cf, key, value);
        }
    }

    fn addr_tx_key(addr: &Address, block_number: u64, tx_index: u32) -> Vec<u8> {
        let mut key = addr.0.to_vec();
        key.extend_from_slice(&block_number.to_be_bytes());
        key.extend_from_slice(&tx_index.to_be_bytes());
        key
    }

    pub fn stats(&self) -> IndexerStats {
        IndexerStats { latest_indexed: self.latest_indexed, total_indexed: self.total_indexed }
    }
}

#[derive(Debug, Clone)]
pub struct IndexerStats {
    pub latest_indexed: u64,
    pub total_indexed: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum IndexerError {
    #[error("Database error: {0}")]
    Database(String),
    #[error("Serialization error: {0}")]
    Serialization(String),
    #[error("Column family not found: {0}")]
    CfNotFound(&'static str),
    #[error("Reorg too deep: depth {depth}, max {max}")]
    ReorgTooDeep { depth: u64, max: u64 },
    #[error("Not found")]
    NotFound,
}