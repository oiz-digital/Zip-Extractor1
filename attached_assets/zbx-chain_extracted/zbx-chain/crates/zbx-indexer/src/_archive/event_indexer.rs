//! Event/Log indexer — efficient bloom filter + topic-based log search.

use std::collections::HashMap;
use crate::types::{Address, U256, BlockHash, TxHash};

/// Bloom filter for log matching (2048-bit Ethereum-style)
#[derive(Debug, Clone, Default)]
pub struct Bloom([u8; 256]);

impl Bloom {
    pub fn new() -> Self { Self([0u8; 256]) }

    /// Add item to bloom filter
    pub fn add(&mut self, item: &[u8]) {
        for i in 0..3usize {
            let hash = self.keccak_hash(item, i);
            let bit = hash & 0x7FF;
            let byte_idx = 255 - (bit / 8) as usize;
            let bit_idx = bit % 8;
            self.0[byte_idx] |= 1 << bit_idx;
        }
    }

    /// Check if item might be in filter
    pub fn contains(&self, item: &[u8]) -> bool {
        for i in 0..3usize {
            let hash = self.keccak_hash(item, i);
            let bit = hash & 0x7FF;
            let byte_idx = 255 - (bit / 8) as usize;
            let bit_idx = bit % 8;
            if self.0[byte_idx] & (1 << bit_idx) == 0 { return false; }
        }
        true
    }

    fn keccak_hash(&self, item: &[u8], seed: usize) -> u64 {
        use sha3::{Keccak256, Digest};
        let mut h = Keccak256::new();
        h.update(item);
        h.update(&[seed as u8]);
        let result = h.finalize();
        u64::from_be_bytes(result[..8].try_into().unwrap())
    }

    /// Merge two bloom filters
    pub fn or_assign(&mut self, other: &Bloom) {
        for i in 0..256 { self.0[i] |= other.0[i]; }
    }

    pub fn to_bytes(&self) -> &[u8; 256] { &self.0 }
    pub fn from_bytes(bytes: [u8; 256]) -> Self { Self(bytes) }
    pub fn is_empty(&self) -> bool { self.0.iter().all(|&b| b == 0) }
}

/// Log entry
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LogEntry {
    pub address: Address,
    pub topics: Vec<U256>,
    pub data: Vec<u8>,
    pub block_number: u64,
    pub block_hash: BlockHash,
    pub tx_hash: TxHash,
    pub tx_index: u32,
    pub log_index: u32,
    pub removed: bool,
}

/// Log filter parameters
#[derive(Debug, Clone, Default)]
pub struct LogFilter {
    pub from_block: Option<u64>,
    pub to_block: Option<u64>,
    pub addresses: Vec<Address>,
    pub topics: Vec<Option<Vec<U256>>>, // None = wildcard
    pub limit: Option<usize>,
}

impl LogFilter {
    pub fn matches(&self, log: &LogEntry) -> bool {
        // Block range
        if let Some(from) = self.from_block { if log.block_number < from { return false; } }
        if let Some(to) = self.to_block { if log.block_number > to { return false; } }
        // Address filter
        if !self.addresses.is_empty() && !self.addresses.contains(&log.address) { return false; }
        // Topic filter
        for (i, filter_topic) in self.topics.iter().enumerate() {
            if let Some(allowed) = filter_topic {
                if i >= log.topics.len() { return false; }
                if !allowed.is_empty() && !allowed.contains(&log.topics[i]) { return false; }
            }
        }
        true
    }

    pub fn build_bloom(&self) -> Bloom {
        let mut bloom = Bloom::new();
        for addr in &self.addresses { bloom.add(&addr.0); }
        for topic_list in &self.topics {
            if let Some(topics) = topic_list {
                for t in topics { bloom.add(&t.to_be_bytes()); }
            }
        }
        bloom
    }
}

/// Event indexer
pub struct EventIndexer {
    /// Per-block bloom filters (block_number -> bloom)
    pub block_blooms: HashMap<u64, Bloom>,
    /// In-memory log buffer (production: use RocksDB)
    pub logs: Vec<LogEntry>,
    pub log_count: u64,
}

impl EventIndexer {
    pub fn new() -> Self {
        Self { block_blooms: HashMap::new(), logs: Vec::new(), log_count: 0 }
    }

    /// Index logs from a block
    pub fn index_block_logs(&mut self, block_number: u64, block_hash: BlockHash, new_logs: Vec<LogEntry>) {
        let mut bloom = Bloom::new();
        for log in &new_logs {
            bloom.add(&log.address.0);
            for topic in &log.topics { bloom.add(&topic.to_be_bytes()); }
        }
        self.block_blooms.insert(block_number, bloom);
        self.log_count += new_logs.len() as u64;
        self.logs.extend(new_logs);
        tracing::debug!(block = block_number, "Logs indexed");
    }

    /// Query logs by filter
    pub fn get_logs(&self, filter: &LogFilter) -> Vec<&LogEntry> {
        let filter_bloom = filter.build_bloom();
        let limit = filter.limit.unwrap_or(10_000);
        let mut results = Vec::new();

        for log in &self.logs {
            // Quick bloom check on block
            if !filter_bloom.is_empty() {
                let block_bloom = match self.block_blooms.get(&log.block_number) {
                    Some(b) => b,
                    None => continue,
                };
                if !self.bloom_matches(&filter_bloom, block_bloom) { continue; }
            }
            if filter.matches(log) {
                results.push(log);
                if results.len() >= limit { break; }
            }
        }
        results
    }

    fn bloom_matches(&self, filter: &Bloom, block: &Bloom) -> bool {
        // Check if all filter bits are set in block bloom
        for i in 0..256 {
            if filter.0[i] & block.0[i] != filter.0[i] { return false; }
        }
        true
    }

    /// Handle reorg: remove logs from blocks above fork_point
    pub fn handle_reorg(&mut self, fork_point: u64) {
        self.block_blooms.retain(|&n, _| n <= fork_point);
        self.logs.retain(|l| l.block_number <= fork_point);
        self.log_count = self.logs.len() as u64;
    }

    pub fn stats(&self) -> EventIndexerStats {
        EventIndexerStats { total_logs: self.log_count, bloom_entries: self.block_blooms.len() }
    }
}

#[derive(Debug, Clone)]
pub struct EventIndexerStats {
    pub total_logs: u64,
    pub bloom_entries: usize,
}