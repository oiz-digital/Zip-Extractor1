//! Search endpoint — look up blocks, transactions, and addresses by hash or number.

use crate::indexer::ExplorerDB;

/// Search result variants returned by the unified search endpoint.
#[derive(Debug, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SearchResult {
    Block   { number: u64, hash: String },
    Tx      { hash: String },
    Address { address: String },
    NotFound,
}

/// Dispatch a free-text query to the appropriate lookup.
pub fn search(query: &str, db: &dyn ExplorerDB) -> SearchResult {
    let q = query.trim();
    if q.is_empty() {
        return SearchResult::NotFound;
    }
    // 0x + 64 hex chars → transaction or block hash
    if q.starts_with("0x") && q.len() == 66 {
        if let Some(block) = db.block_by_hash(q) {
            return SearchResult::Block { number: block.number, hash: block.hash };
        }
        return SearchResult::Tx { hash: q.to_owned() };
    }
    // 0x + 40 hex chars → address
    if q.starts_with("0x") && q.len() == 42 {
        return SearchResult::Address { address: q.to_owned() };
    }
    // pure number → block by number
    if let Ok(n) = q.parse::<u64>() {
        if let Some(block) = db.block_by_number(n) {
            return SearchResult::Block { number: block.number, hash: block.hash };
        }
    }
    SearchResult::NotFound
}
