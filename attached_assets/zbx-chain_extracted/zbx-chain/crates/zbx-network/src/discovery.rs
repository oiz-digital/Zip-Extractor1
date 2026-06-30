//! Kademlia-based peer discovery with a k-bucket routing table.

use crate::{error::NetworkError, peer::PeerId};
use std::collections::BTreeMap;
use zbx_crypto::keccak::keccak256;

const K: usize = 16; // k-bucket size
const ALPHA: usize = 3; // concurrency parameter

/// XOR distance between two peer IDs.
fn xor_distance(a: &PeerId, b: &PeerId) -> [u8; 32] {
    let mut d = [0u8; 32];
    for i in 0..32 {
        d[i] = a.0[i] ^ b.0[i];
    }
    d
}

/// Bit-length of the common prefix between two peer IDs.
fn common_prefix_len(a: &PeerId, b: &PeerId) -> usize {
    for i in 0..32 {
        let x = a.0[i] ^ b.0[i];
        if x != 0 {
            return i * 8 + x.leading_zeros() as usize;
        }
    }
    256
}

/// A single k-bucket holding at most K peers.
struct KBucket {
    peers: Vec<PeerId>,
}

impl KBucket {
    fn new() -> Self { KBucket { peers: Vec::with_capacity(K) } }

    fn add(&mut self, id: PeerId) -> bool {
        if self.peers.contains(&id) {
            // Move to tail (most-recently-seen)
            self.peers.retain(|p| p != &id);
            self.peers.push(id);
            return true;
        }
        if self.peers.len() < K {
            self.peers.push(id);
            return true;
        }
        false // bucket full — ping head before evicting
    }

    fn remove(&mut self, id: &PeerId) {
        self.peers.retain(|p| p != id);
    }

    fn closest(&self, _target: &PeerId) -> Vec<&PeerId> {
        self.peers.iter().collect()
    }
}

/// Kademlia routing table with 256 k-buckets.
pub struct RoutingTable {
    local: PeerId,
    buckets: Vec<KBucket>,
}

impl RoutingTable {
    pub fn new(local: PeerId) -> Self {
        let buckets = (0..256).map(|_| KBucket::new()).collect();
        RoutingTable { local, buckets }
    }

    fn bucket_index(&self, id: &PeerId) -> usize {
        let cpl = common_prefix_len(&self.local, id);
        cpl.min(255)
    }

    pub fn add(&mut self, id: PeerId) -> bool {
        if id == self.local { return false; }
        let idx = self.bucket_index(&id);
        self.buckets[idx].add(id)
    }

    pub fn remove(&mut self, id: &PeerId) {
        let idx = self.bucket_index(id);
        self.buckets[idx].remove(id);
    }

    /// Find the K closest known peers to the target.
    pub fn closest_peers(&self, target: &PeerId) -> Vec<PeerId> {
        let mut candidates: Vec<(PeerId, [u8; 32])> = self
            .buckets
            .iter()
            .flat_map(|b| b.closest(target))
            .map(|id| (id.clone(), xor_distance(id, target)))
            .collect();
        candidates.sort_by(|a, b| a.1.cmp(&b.1));
        candidates.into_iter().take(K).map(|(id, _)| id).collect()
    }

    pub fn total_peers(&self) -> usize {
        self.buckets.iter().map(|b| b.peers.len()).sum()
    }
}