//! Pay ID registry cache — avoids repeated RPC calls for hot Pay IDs.
//!
//! Every cached entry stores both the wallet address AND the display name
//! (e.g. "Salman Tyagi") — both are mandatory at registration time.

use crate::{resolver::ResolvedPayId};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

struct CacheEntry {
    resolved:  ResolvedPayId,   // includes display_name field
    cached_at: Instant,
}

/// In-memory LRU-like cache for Pay ID resolutions.
pub struct PayIdRegistry {
    cache: Arc<RwLock<HashMap<String, CacheEntry>>>,
    /// Cache entry TTL (default 5 minutes).
    ttl: Duration,
}

impl PayIdRegistry {
    pub fn new() -> Self {
        PayIdRegistry {
            cache: Arc::new(RwLock::new(HashMap::new())),
            ttl: Duration::from_secs(300),
        }
    }

    /// Get a cached resolution (None if expired or missing).
    pub fn get(&self, pay_id: &str) -> Option<ResolvedPayId> {
        let cache = self.cache.read().unwrap();
        if let Some(entry) = cache.get(pay_id) {
            if entry.cached_at.elapsed() < self.ttl {
                return Some(entry.resolved.clone());
            }
        }
        None
    }

    /// Cache a resolved Pay ID.
    pub fn insert(&self, pay_id: String, resolved: ResolvedPayId) {
        self.cache.write().unwrap().insert(pay_id, CacheEntry {
            resolved,
            cached_at: Instant::now(),
        });
    }

    /// Invalidate a cache entry (call after wallet update).
    pub fn invalidate(&self, pay_id: &str) {
        self.cache.write().unwrap().remove(pay_id);
    }

    pub fn len(&self) -> usize {
        self.cache.read().unwrap().len()
    }
}

impl Default for PayIdRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// Task #3 (Precompile 0x0A — PayID resolution): the on-chain registry
// layout helpers + lookup trait live in `zbx-types::payid` so leaf VM
// crates can use them without pulling our `reqwest` / `tokio` runtime
// deps in. Re-exported here for legacy callers.
pub use zbx_types::payid::{
    payid_forward_slot, payid_reverse_slot, validate_payid_name,
    PayIdLookup, PAYID_REGISTRAR_ADDR,
};