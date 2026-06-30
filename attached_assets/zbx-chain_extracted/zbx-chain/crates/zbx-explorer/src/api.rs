//! HTTP API handlers for the block explorer.

use crate::indexer::{ExplorerDB, ExplorerBlock, ExplorerTx};
use crate::search::{search, SearchResult};

/// Query a block by number or hash.
pub fn get_block<'a>(db: &'a dyn ExplorerDB, key: &str) -> Option<ExplorerBlock> {
    if let Ok(num) = key.parse::<u64>() {
        db.block_by_number(num)
    } else {
        db.block_by_hash(key)
    }
}

/// Query a transaction by hash.
pub fn get_tx<'a>(db: &'a dyn ExplorerDB, hash: &str) -> Option<ExplorerTx> {
    db.tx_by_hash(hash)
}

/// Unified search over blocks, transactions, and addresses.
pub fn handle_search(query: &str, db: &dyn ExplorerDB) -> SearchResult {
    search(query, db)
}

/// Returns the current chain tip block number.
pub fn chain_tip(db: &dyn ExplorerDB) -> u64 {
    db.latest_block_number()
}
