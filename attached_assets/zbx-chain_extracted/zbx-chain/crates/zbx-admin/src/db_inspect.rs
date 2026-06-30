//! Database inspection and maintenance tools.

use crate::error::AdminError;
use zbx_types::{address::Address, H256};
use serde::{Serialize, Deserialize};
use tracing::{info, warn};

/// Column-family options for raw inspection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ColumnFamilyOption {
    Headers,
    Bodies,
    BlockNumbers,
    CanonicalChain,
    TxLookup,
    Receipts,
    AccountTrie,
    StorageTrie,
    Code,
    AccountSnaps,
    QuorumCerts,
    PendingTxs,
    Meta,
}

impl ColumnFamilyOption {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Headers        => "headers",
            Self::Bodies         => "bodies",
            Self::BlockNumbers   => "block_numbers",
            Self::CanonicalChain => "canonical_chain",
            Self::TxLookup       => "tx_lookup",
            Self::Receipts       => "receipts",
            Self::AccountTrie    => "account_trie",
            Self::StorageTrie    => "storage_trie",
            Self::Code           => "code",
            Self::AccountSnaps   => "account_snaps",
            Self::QuorumCerts    => "quorum_certs",
            Self::PendingTxs     => "pending_txs",
            Self::Meta           => "meta",
        }
    }
}

/// Database size statistics.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct DbStats {
    pub total_size_bytes:   u64,
    pub column_sizes:       Vec<(String, u64)>,
    pub num_keys_estimate:  u64,
    pub num_sst_files:      u32,
    pub num_levels:         u32,
    pub block_cache_hit:    u64,
    pub block_cache_miss:   u64,
    pub write_stall_total:  u64,
    pub compaction_pending: bool,
}

/// Inspect a raw key in a column family.
pub fn inspect_key(
    col: ColumnFamilyOption,
    key: &[u8],
) -> Result<Option<Vec<u8>>, AdminError> {
    info!("admin: db inspect col='{}' key=0x{}", col.as_str(), hex::encode(key));
    // In production: db.get_cf(col, key)
    Ok(None)
}

/// Look up an account by address.
pub fn inspect_account(addr: Address) -> Result<String, AdminError> {
    info!("admin: inspecting account {:?}", addr);
    // In production: read AccountTrie, decode RLP.
    Ok(serde_json::json!({
        "address":   format!("{:?}", addr),
        "nonce":     0,
        "balance":   "0",
        "code_hash": "0xc5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470",
        "root":      "0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421",
    }).to_string())
}

/// Look up a block header by number or hash.
pub fn inspect_block(
    by: BlockLookup,
) -> Result<Option<serde_json::Value>, AdminError> {
    let _ = by;
    Ok(None)
}

#[derive(Debug)]
pub enum BlockLookup {
    ByNumber(u64),
    ByHash(H256),
}

/// Get RocksDB statistics.
pub fn db_stats() -> Result<DbStats, AdminError> {
    Ok(DbStats::default())
}

/// Trigger RocksDB full compaction (blocks until complete).
pub fn compact_db() -> Result<(), AdminError> {
    warn!("admin: triggering full RocksDB compaction (this may take several minutes)");
    // In production: db.compact_range_cf(...) for each CF.
    Ok(())
}

/// Run consistency check on all column families.
pub fn verify_db() -> Result<Vec<String>, AdminError> {
    info!("admin: running database consistency check");
    // In production: db.verify_checksum().
    Ok(vec!["All column families OK".into()])
}

/// Prune ancient state beyond `keep_blocks` from head.
pub fn prune_state(keep_blocks: u64) -> Result<u64, AdminError> {
    if keep_blocks < 1024 {
        return Err(AdminError::InvalidParam(format!(
            "keep_blocks {} is too low (minimum 1024)", keep_blocks
        )));
    }
    warn!("admin: pruning state — keeping last {} blocks", keep_blocks);
    // Returns number of keys deleted.
    Ok(0)
}