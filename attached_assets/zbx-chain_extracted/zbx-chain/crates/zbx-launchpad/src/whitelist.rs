//! Whitelist management for IDO pools.

use std::collections::{HashMap, HashSet};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Whitelist {
    pool_id:    u64,
    addresses:  HashSet<[u8; 20]>,
    /// Per-address max contribution cap (0 = unlimited)
    caps:       HashMap<[u8; 20], u128>,
}

impl Whitelist {
    pub fn new(pool_id: u64) -> Self {
        Self { pool_id, addresses: HashSet::new(), caps: HashMap::new() }
    }

    pub fn add(&mut self, addr: [u8; 20], cap: u128) {
        self.addresses.insert(addr);
        if cap > 0 { self.caps.insert(addr, cap); }
    }

    pub fn remove(&mut self, addr: &[u8; 20]) {
        self.addresses.remove(addr);
        self.caps.remove(addr);
    }

    pub fn is_allowed(&self, addr: &[u8; 20]) -> bool {
        self.addresses.contains(addr)
    }

    pub fn cap_for(&self, addr: &[u8; 20]) -> Option<u128> {
        self.caps.get(addr).copied()
    }

    pub fn len(&self) -> usize { self.addresses.len() }
    pub fn is_empty(&self) -> bool { self.addresses.is_empty() }
}
