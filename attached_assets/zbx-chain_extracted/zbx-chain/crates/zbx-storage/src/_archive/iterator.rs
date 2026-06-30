//! Range iterators over RocksDB column families.

use crate::{error::StorageError, schema::Column};

/// Direction for iteration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IteratorMode {
    Start,
    End,
    From(Vec<u8>, Direction),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction { Forward, Reverse }

/// A key-value item returned by a `DbIterator`.
pub type KvItem = Result<(Vec<u8>, Vec<u8>), StorageError>;

/// An iterator over a column family range.
pub struct DbIterator {
    items: std::collections::VecDeque<KvItem>,
    prefix: Option<Vec<u8>>,
}

impl DbIterator {
    pub fn empty() -> Self {
        Self { items: Default::default(), prefix: None }
    }

    pub fn from_vec(items: Vec<(Vec<u8>, Vec<u8>)>) -> Self {
        Self {
            items: items.into_iter().map(|(k, v)| Ok((k, v))).collect(),
            prefix: None,
        }
    }

    /// Filter to only return items whose key starts with `prefix`.
    pub fn with_prefix(mut self, prefix: Vec<u8>) -> Self {
        self.prefix = Some(prefix); self
    }
}

impl Iterator for DbIterator {
    type Item = KvItem;
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let item = self.items.pop_front()?;
            if let Some(ref pfx) = self.prefix {
                match &item {
                    Ok((k, _)) if !k.starts_with(pfx) => continue,
                    _ => {}
                }
            }
            return Some(item);
        }
    }
}

/// Configuration for a range scan.
#[derive(Debug, Clone, Default)]
pub struct ScanConfig {
    pub column:     Option<Column>,
    pub from:       Option<Vec<u8>>,
    pub to:         Option<Vec<u8>>,
    pub prefix:     Option<Vec<u8>>,
    pub limit:      Option<usize>,
    pub reverse:    bool,
    pub keys_only:  bool,
}

impl ScanConfig {
    pub fn new() -> Self { Self::default() }
    pub fn column(mut self, c: Column) -> Self { self.column = Some(c); self }
    pub fn from(mut self, k: Vec<u8>) -> Self { self.from = Some(k); self }
    pub fn to(mut self, k: Vec<u8>) -> Self { self.to = Some(k); self }
    pub fn prefix(mut self, p: Vec<u8>) -> Self { self.prefix = Some(p); self }
    pub fn limit(mut self, n: usize) -> Self { self.limit = Some(n); self }
    pub fn reverse(mut self) -> Self { self.reverse = true; self }
    pub fn keys_only(mut self) -> Self { self.keys_only = true; self }
}