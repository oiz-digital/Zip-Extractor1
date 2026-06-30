//! Atomic write batch for the storage layer.

use crate::schema::Column;

/// A set of key-value writes to be applied atomically.
pub struct WriteBatch {
    /// (column, key, value) triples
    pub ops: Vec<(Column, Vec<u8>, Vec<u8>)>,
}

impl WriteBatch {
    pub fn new() -> Self {
        WriteBatch { ops: Vec::new() }
    }

    pub fn put(&mut self, col: Column, key: Vec<u8>, value: Vec<u8>) {
        self.ops.push((col, key, value));
    }

    pub fn delete(&mut self, col: Column, key: Vec<u8>) {
        // Represented as a put of empty value; the db layer interprets this as delete.
        self.ops.push((col, key, Vec::new()));
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    pub fn len(&self) -> usize {
        self.ops.len()
    }
}

impl Default for WriteBatch {
    fn default() -> Self { Self::new() }
}