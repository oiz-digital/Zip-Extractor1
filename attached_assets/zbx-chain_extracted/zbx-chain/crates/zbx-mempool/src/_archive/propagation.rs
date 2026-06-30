//! Transaction propagation over P2P network.
//! Implements EIP-5793 (tx batcher), flood control, peer selection.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::types::{TxHash, Address};
use crate::mempool::PendingTransaction;

/// Max transactions per announcement batch
pub const MAX_TX_ANNOUNCEMENT_BATCH: usize = 256;
/// Announcement interval
pub const ANNOUNCEMENT_INTERVAL: Duration = Duration::from_millis(500);
/// Max pending announcements per peer
pub const MAX_PENDING_ANNOUNCEMENTS_PER_PEER: usize = 4096;
/// Full tx send threshold (small txs sent directly, large only announced)
pub const FULL_TX_SIZE_THRESHOLD: usize = 128 * 1024; // 128 KB
/// Max re-announcement count per tx
pub const MAX_REANNOUNCE_COUNT: usize = 3;

/// Propagation manager
pub struct PropagationManager {
    /// Per-peer announcement queues
    pub announcement_queues: HashMap<PeerId, AnnouncementQueue>,
    /// Transactions already sent to each peer
    pub sent_to: HashMap<PeerId, HashSet<TxHash>>,
    /// Tx metadata for propagation decisions
    pub tx_meta: HashMap<TxHash, TxPropMeta>,
    /// Flood limiter per peer
    pub flood_limiter: HashMap<PeerId, FloodLimiter>,
    /// Batch sender
    pub batch_sender: BatchSender,
    /// Config
    pub config: PropagationConfig,
}

pub type PeerId = [u8; 32];

/// Per-tx propagation metadata
#[derive(Debug, Clone)]
pub struct TxPropMeta {
    pub hash: TxHash,
    pub size: usize,
    pub sender: Address,
    pub announce_count: usize,
    pub last_announced_at: Instant,
    pub full_sent_to: HashSet<PeerId>,
}

/// Announcement queue per peer
#[derive(Debug)]
pub struct AnnouncementQueue {
    pub peer: PeerId,
    pub pending: VecDeque<TxHash>,
    pub max_size: usize,
    pub last_flush: Instant,
}

impl AnnouncementQueue {
    pub fn new(peer: PeerId) -> Self {
        Self { peer, pending: VecDeque::new(), max_size: MAX_PENDING_ANNOUNCEMENTS_PER_PEER, last_flush: Instant::now() }
    }
    pub fn enqueue(&mut self, hash: TxHash) {
        if self.pending.len() < self.max_size { self.pending.push_back(hash); }
    }
    pub fn flush(&mut self) -> Vec<TxHash> {
        let take = self.pending.len().min(MAX_TX_ANNOUNCEMENT_BATCH);
        self.pending.drain(..take).collect()
    }
    pub fn should_flush(&self) -> bool {
        self.pending.len() >= MAX_TX_ANNOUNCEMENT_BATCH
            || self.last_flush.elapsed() >= ANNOUNCEMENT_INTERVAL
    }
}

/// Per-peer flood limiter
#[derive(Debug)]
pub struct FloodLimiter {
    pub peer: PeerId,
    pub bucket: f64,      // token bucket
    pub max_bucket: f64,
    pub refill_rate: f64, // tokens per second
    pub last_refill: Instant,
}

impl FloodLimiter {
    pub fn new(peer: PeerId, rate: f64) -> Self {
        Self { peer, bucket: rate, max_bucket: rate * 2.0, refill_rate: rate, last_refill: Instant::now() }
    }
    pub fn try_consume(&mut self, tokens: f64) -> bool {
        self.refill();
        if self.bucket >= tokens {
            self.bucket -= tokens;
            true
        } else {
            false
        }
    }
    fn refill(&mut self) {
        let elapsed = self.last_refill.elapsed().as_secs_f64();
        self.bucket = (self.bucket + elapsed * self.refill_rate).min(self.max_bucket);
        self.last_refill = Instant::now();
    }
}

/// Batch sender (groups announcements into network packets)
pub struct BatchSender {
    pub pending_batches: VecDeque<AnnouncementBatch>,
    pub max_batch_size: usize,
}

#[derive(Debug, Clone)]
pub struct AnnouncementBatch {
    pub hashes: Vec<TxHash>,
    pub peer: PeerId,
    pub created_at: Instant,
}

/// Propagation configuration
#[derive(Debug, Clone)]
pub struct PropagationConfig {
    pub flood_rate_per_peer: f64,    // tx/sec
    pub max_peers_broadcast: usize,
    pub use_eth68: bool,              // EIP-5793 announcement protocol
    pub full_tx_threshold_bytes: usize,
}

impl Default for PropagationConfig {
    fn default() -> Self {
        Self {
            flood_rate_per_peer: 100.0,
            max_peers_broadcast: 25,
            use_eth68: true,
            full_tx_threshold_bytes: FULL_TX_SIZE_THRESHOLD,
        }
    }
}

impl PropagationManager {
    pub fn new(config: PropagationConfig) -> Self {
        Self {
            announcement_queues: HashMap::new(),
            sent_to: HashMap::new(),
            tx_meta: HashMap::new(),
            flood_limiter: HashMap::new(),
            batch_sender: BatchSender { pending_batches: VecDeque::new(), max_batch_size: MAX_TX_ANNOUNCEMENT_BATCH },
            config,
        }
    }

    /// Enqueue a new transaction for propagation
    pub fn enqueue(&mut self, tx: &PendingTransaction, connected_peers: &[PeerId]) {
        let hash = tx.tx.hash();
        let size = tx.tx.size();
        let meta = TxPropMeta {
            hash,
            size,
            sender: tx.tx.recover_sender().unwrap_or_default(),
            announce_count: 0,
            last_announced_at: Instant::now(),
            full_sent_to: HashSet::new(),
        };
        self.tx_meta.insert(hash, meta);

        // Select peers to propagate to (max sqrt(n) for full, rest get announcement)
        let full_peers_count = (connected_peers.len() as f64).sqrt().ceil() as usize;
        let (full_peers, announce_peers) = connected_peers.split_at(full_peers_count.min(connected_peers.len()));

        // Send full tx to sqrt(n) peers
        for peer in full_peers {
            if size < self.config.full_tx_threshold_bytes {
                self.schedule_full(*peer, hash);
            } else {
                self.enqueue_announcement(*peer, hash);
            }
        }

        // Announce to the rest
        for peer in announce_peers {
            if self.config.use_eth68 {
                self.enqueue_announcement(*peer, hash);
            } else {
                self.schedule_full(*peer, hash);
            }
        }
    }

    fn schedule_full(&mut self, peer: PeerId, hash: TxHash) {
        self.sent_to.entry(peer).or_default().insert(hash);
    }

    fn enqueue_announcement(&mut self, peer: PeerId, hash: TxHash) {
        self.announcement_queues.entry(peer).or_insert_with(|| AnnouncementQueue::new(peer)).enqueue(hash);
    }

    /// Flush announcement queues for all ready peers
    pub fn flush_announcements(&mut self) -> Vec<AnnouncementBatch> {
        let mut batches = Vec::new();
        for (peer, queue) in self.announcement_queues.iter_mut() {
            if queue.should_flush() {
                let hashes = queue.flush();
                if !hashes.is_empty() {
                    batches.push(AnnouncementBatch { hashes, peer: *peer, created_at: Instant::now() });
                }
                queue.last_flush = Instant::now();
            }
        }
        batches
    }

    /// Add a peer connection
    pub fn add_peer(&mut self, peer: PeerId, rate: f64) {
        self.announcement_queues.insert(peer, AnnouncementQueue::new(peer));
        self.flood_limiter.insert(peer, FloodLimiter::new(peer, rate));
    }

    /// Remove a disconnected peer
    pub fn remove_peer(&mut self, peer: &PeerId) {
        self.announcement_queues.remove(peer);
        self.flood_limiter.remove(peer);
        self.sent_to.remove(peer);
    }
}