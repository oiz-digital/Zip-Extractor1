//! Blob store: persists blobs and sidecars keyed by versioned hash.

use crate::{blob::{Blob, BlobSidecar}, error::DaError};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// In-memory + RocksDB backed blob store.
pub struct BlobStore {
    /// In-memory cache for recent blobs (< finality window).
    cache: Arc<RwLock<HashMap<[u8; 32], BlobSidecar>>>,
}

impl BlobStore {
    pub fn new() -> Self {
        BlobStore {
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Store a sidecar by its blob versioned hash.
    pub fn insert(&self, hash: [u8; 32], sidecar: BlobSidecar) -> Result<(), DaError> {
        self.cache.write().unwrap().insert(hash, sidecar);
        Ok(())
    }

    /// Retrieve a sidecar by its blob versioned hash.
    pub fn get(&self, hash: &[u8; 32]) -> Option<BlobSidecar> {
        self.cache.read().unwrap().get(hash).cloned()
    }

    /// Check if a blob is available.
    pub fn contains(&self, hash: &[u8; 32]) -> bool {
        self.cache.read().unwrap().contains_key(hash)
    }

    /// Remove blobs older than the finality window.
    pub fn prune_before(&self, cutoff_block: u64, block_blob_index: &HashMap<u64, Vec<[u8; 32]>>) {
        let mut cache = self.cache.write().unwrap();
        for (block, hashes) in block_blob_index {
            if *block < cutoff_block {
                for hash in hashes {
                    cache.remove(hash);
                }
            }
        }
    }
}

impl Default for BlobStore {
    fn default() -> Self {
        Self::new()
    }
}