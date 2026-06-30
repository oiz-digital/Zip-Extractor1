//! Message cache: deduplication sliding window for GossipSub.

use std::collections::{HashMap, VecDeque};
use zbx_types::H256;

/// A cached gossip message.
#[derive(Debug, Clone)]
pub struct CachedMessage {
    pub id:    H256,
    pub topic: String,
    pub data:  Vec<u8>,
    pub slot:  usize,
}

/// Sliding-window message cache.
pub struct MessageCache {
    /// Messages indexed by ID.
    by_id: HashMap<H256, CachedMessage>,
    /// Ordered list of (slot, id) for window eviction.
    window: VecDeque<(usize, H256)>,
    /// Current heartbeat slot.
    current_slot: usize,
    /// Number of slots to retain.
    history_len: usize,
    /// Max entries (LRU bound).
    max_entries: usize,
}

impl MessageCache {
    pub fn new(history_len: usize, max_entries: usize) -> Self {
        Self {
            by_id: HashMap::new(),
            window: VecDeque::new(),
            current_slot: 0,
            history_len,
            max_entries,
        }
    }

    /// Check if a message ID was already seen.
    pub fn seen(&self, id: &H256) -> bool {
        self.by_id.contains_key(id)
    }

    /// Insert a new message. Returns false if duplicate.
    pub fn insert(&mut self, msg: CachedMessage) -> bool {
        if self.by_id.contains_key(&msg.id) { return false; }
        // LRU eviction.
        if self.by_id.len() >= self.max_entries {
            if let Some((_, old_id)) = self.window.pop_front() {
                self.by_id.remove(&old_id);
            }
        }
        let id = msg.id;
        let slot = self.current_slot;
        self.window.push_back((slot, id));
        self.by_id.insert(id, msg);
        true
    }

    /// Advance to the next heartbeat slot, evicting old messages.
    pub fn advance_slot(&mut self) {
        self.current_slot += 1;
        let cutoff = self.current_slot.saturating_sub(self.history_len);
        while let Some(&(slot, id)) = self.window.front() {
            if slot < cutoff {
                self.window.pop_front();
                self.by_id.remove(&id);
            } else {
                break;
            }
        }
    }

    /// Get all message IDs in the last `gossip_slots` slots (for IHAVE).
    pub fn get_gossip_ids(&self, gossip_slots: usize) -> Vec<H256> {
        let cutoff = self.current_slot.saturating_sub(gossip_slots);
        self.window.iter()
            .filter(|(slot, _)| *slot >= cutoff)
            .map(|(_, id)| *id)
            .collect()
    }

    pub fn get(&self, id: &H256) -> Option<&CachedMessage> {
        self.by_id.get(id)
    }

    pub fn len(&self) -> usize { self.by_id.len() }

    /// Return the current heartbeat slot number.
    /// Used by GossipManager to stamp inserted messages with the correct slot
    /// so that advance_slot() can evict them when the window moves forward.
    pub fn current_slot(&self) -> usize { self.current_slot }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn h(b: u8) -> H256 { [b; 32] }

    fn msg(id: H256) -> CachedMessage {
        CachedMessage { id, topic: "blocks".into(), data: vec![1, 2, 3], slot: 0 }
    }

    #[test]
    fn insert_and_seen() {
        let mut cache = MessageCache::new(4, 128);
        assert!(!cache.seen(&h(1)));
        assert!(cache.insert(msg(h(1))));
        assert!(cache.seen(&h(1)));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn duplicate_insert_returns_false() {
        let mut cache = MessageCache::new(4, 128);
        assert!(cache.insert(msg(h(1))));
        assert!(!cache.insert(msg(h(1))));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn lru_eviction_at_max_entries() {
        let mut cache = MessageCache::new(4, 2);
        cache.insert(msg(h(1)));
        cache.insert(msg(h(2)));
        cache.insert(msg(h(3)));
        assert_eq!(cache.len(), 2);
        assert!(!cache.seen(&h(1)));
    }

    #[test]
    fn get_returns_message() {
        let mut cache = MessageCache::new(4, 128);
        cache.insert(msg(h(7)));
        assert!(cache.get(&h(7)).is_some());
        assert!(cache.get(&h(9)).is_none());
    }

    #[test]
    fn get_gossip_ids_within_window() {
        let mut cache = MessageCache::new(4, 128);
        cache.insert(msg(h(1)));
        let ids = cache.get_gossip_ids(2);
        assert!(ids.contains(&h(1)));
    }

    #[test]
    fn current_slot_initially_zero() {
        let cache = MessageCache::new(4, 128);
        assert_eq!(cache.current_slot(), 0);
    }
}
