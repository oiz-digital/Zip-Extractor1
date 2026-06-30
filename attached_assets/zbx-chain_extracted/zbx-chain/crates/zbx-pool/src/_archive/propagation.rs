//! Mempool tx propagation over P2P network.
//!
//! ZBX uses two-phase tx propagation (same as Ethereum post-EIP-2464):
//!
//! Phase 1 -- Announcement (hashes only):
//!   Node A receives new tx -> announces tx hash to peers
//!   Peers check if they already have it (seen_tx cache)
//!   If not seen -> request full tx via GetPooledTransactions
//!
//! Phase 2 -- Fetch on demand:
//!   Peer sends GetPooledTransactions(hashes)
//!   Node responds with PooledTransactions(raw_txs)
//!
//! This avoids sending full tx bodies to peers that already have it.
//!
//! Gossip topic: "/zbx/mempool/txs/1"
//! Max peers per broadcast: 8 (sqrt of typical ~64 peers)
//! Seen tx cache TTL: 5 minutes (covers finality window)

use std::collections::{HashMap, HashSet, VecDeque};

// ── Gossip topic ──────────────────────────────────────────────────────────────

/// libp2p gossipsub topic for mempool transactions.
pub const MEMPOOL_GOSSIP_TOPIC: &str = "/zbx/mempool/txs/1";

/// Alternative tx announcement topic (hash-only, EIP-2464 style).
pub const TX_ANNOUNCEMENT_TOPIC: &str = "/zbx/mempool/announce/1";

// ── P2P message types ─────────────────────────────────────────────────────────

/// NewPooledTransactionHashes -- announce tx hashes to peer (EIP-2464).
/// Peers use this to know about new txs without receiving full bodies.
#[derive(Debug, Clone)]
pub struct NewPooledTxHashes {
    /// Transaction hashes being announced (max 256 per message)
    pub hashes: Vec<[u8; 32]>,
    /// Types of the txs (0 = legacy, 2 = EIP-1559) -- EIP-2481
    pub types:  Vec<u8>,
    /// Max sizes of each tx (for memory accounting) -- EIP-2481
    pub sizes:  Vec<u32>,
}

/// GetPooledTransactions -- request full tx bodies by hash.
#[derive(Debug, Clone)]
pub struct GetPooledTransactions {
    /// Request ID (for matching response)
    pub request_id: u64,
    /// Hashes of requested transactions
    pub hashes: Vec<[u8; 32]>,
}

/// PooledTransactions -- response with full tx bodies.
#[derive(Debug, Clone)]
pub struct PooledTransactions {
    /// Matching request ID
    pub request_id: u64,
    /// RLP-encoded transactions (same order as request)
    pub txs: Vec<Vec<u8>>,
}

// ── New tx event ──────────────────────────────────────────────────────────────

/// Event fired when a new valid transaction is added to the pool.
/// Subscribers: P2P propagation, JSON-RPC subscription, MEV searchers.
#[derive(Debug, Clone)]
pub enum TxEvent {
    /// New transaction added to pending pool
    NewPendingTx {
        hash:    [u8; 32],
        from:    [u8; 20],
        raw:     Vec<u8>,
    },
    /// Transaction removed (mined / replaced / expired)
    TxRemoved {
        hash:   [u8; 32],
        reason: TxRemoveReason,
    },
}

#[derive(Debug, Clone)]
pub enum TxRemoveReason {
    Mined,
    ReplacedByFee,
    Expired,
    Evicted,
    Invalid,
}

// ── Seen tx cache ─────────────────────────────────────────────────────────────

/// Seen tx cache -- tracks recently announced tx hashes to avoid re-broadcast.
///
/// When we receive NewPooledTxHashes from peer A:
///   - Mark hashes as seen-from-A
///   - Do NOT re-announce to A (would be redundant)
///   - Announce to other peers that haven't seen it
///
/// TTL: 5 minutes (transactions expire from mempool after 5 min if not mined)
pub struct SeenTxCache {
    /// hash -> set of peer IDs that announced it to us
    pub seen:    HashMap<[u8; 32], HashSet<String>>,
    /// FIFO queue for TTL-based eviction (hash, received_at_secs)
    pub expiry:  VecDeque<([u8; 32], u64)>,
    /// Cache TTL in seconds
    pub ttl_secs: u64,
}

impl SeenTxCache {
    pub fn new(ttl_secs: u64) -> Self {
        Self { seen: HashMap::new(), expiry: VecDeque::new(), ttl_secs }
    }

    /// Mark a tx hash as seen from a peer. Returns true if this is first time we see it.
    pub fn mark_seen(&mut self, hash: [u8; 32], from_peer: &str, now_secs: u64) -> bool {
        self.evict_expired(now_secs);
        let first_time = !self.seen.contains_key(&hash);
        let peers = self.seen.entry(hash).or_insert_with(HashSet::new);
        peers.insert(from_peer.to_string());
        if first_time { self.expiry.push_back((hash, now_secs + self.ttl_secs)); }
        first_time
    }

    /// Check if a peer has already seen this hash (skip announcing to them).
    pub fn peer_has_seen(&self, hash: &[u8; 32], peer: &str) -> bool {
        self.seen.get(hash).map(|peers| peers.contains(peer)).unwrap_or(false)
    }

    /// Evict expired entries.
    fn evict_expired(&mut self, now_secs: u64) {
        while let Some((hash, exp)) = self.expiry.front() {
            if *exp > now_secs { break; }
            self.seen.remove(hash);
            self.expiry.pop_front();
        }
    }
}

// ── Tx relay policy ───────────────────────────────────────────────────────────

/// Policy determining which transactions should be relayed to peers.
#[derive(Debug, Clone)]
pub struct TxRelayPolicy {
    /// Minimum gas price to relay (don't relay dust txs)
    pub min_relay_gas_price: u128,
    /// Whether to relay txs with zero gas price (private / sponsored txs)
    pub relay_zero_gas:      bool,
    /// Whether to relay large txs (> 128KB)
    pub relay_large_txs:     bool,
    /// Max tx data size to relay (default 128KB)
    pub max_relay_tx_size:   usize,
}

impl Default for TxRelayPolicy {
    fn default() -> Self {
        Self {
            min_relay_gas_price: 1_000_000_000, // 1 gwei
            relay_zero_gas:      false,
            relay_large_txs:     false,
            max_relay_tx_size:   128 * 1024,
        }
    }
}

impl TxRelayPolicy {
    /// Returns true if this transaction should be relayed to peers.
    pub fn should_relay(&self, tx_gas_price: u128, tx_size: usize) -> bool {
        if tx_size > self.max_relay_tx_size && !self.relay_large_txs { return false; }
        if tx_gas_price == 0 { return self.relay_zero_gas; }
        tx_gas_price >= self.min_relay_gas_price
    }
}

// ── Gossip flood limit ────────────────────────────────────────────────────────

/// Gossip flooding limiter for tx propagation.
///
/// Prevents a single peer from flooding us with low-quality transactions.
/// Budget: each peer gets MAX_TX_PER_PEER_PER_SECOND tx accepts per second.
///
/// Algorithm: token bucket per peer.
///   - Bucket refills at REFILL_RATE tokens/second
///   - Each accepted tx costs 1 token
///   - If bucket is empty, tx is dropped (not added to pool)
pub const MAX_TX_PER_PEER_PER_SECOND: u32 = 100;
pub const FLOOD_BUCKET_CAPACITY:      u32 = 500; // burst allowance

/// Per-peer token bucket for flood protection.
pub struct PeerFloodBucket {
    /// Tokens remaining
    pub tokens:      f64,
    /// Last refill time (Unix secs)
    pub last_refill: u64,
}

impl PeerFloodBucket {
    pub fn new(now: u64) -> Self {
        Self { tokens: FLOOD_BUCKET_CAPACITY as f64, last_refill: now }
    }

    /// Try to consume one token. Returns false if rate limit exceeded.
    pub fn try_consume(&mut self, now: u64) -> bool {
        let elapsed = (now.saturating_sub(self.last_refill)) as f64;
        let refill = elapsed * MAX_TX_PER_PEER_PER_SECOND as f64;
        self.tokens = (self.tokens + refill).min(FLOOD_BUCKET_CAPACITY as f64);
        self.last_refill = now;

        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false // flood limit hit
        }
    }
}

/// Global gossip flood limit: max tx we process from ALL peers per second.
pub const GLOBAL_FLOOD_LIMIT: u32 = 10_000;

// ── Tx pool P2P sync on peer connect ─────────────────────────────────────────

/// When a new peer connects, share a sample of our pending pool with them.
///
/// We don't send all txs (too expensive for large pools).
/// Instead we send up to MAX_SYNC_TXS hashes via NewPooledTxHashes.
/// Peer fetches the ones they don't have.
pub const MAX_SYNC_TXS: usize = 256;

/// Announce pending pool to a newly connected peer.
pub fn sync_pool_to_peer(
    pending_hashes: &[[u8; 32]],
    peer_id:        &str,
    seen_cache:     &SeenTxCache,
) -> NewPooledTxHashes {
    let hashes: Vec<[u8; 32]> = pending_hashes.iter()
        .filter(|h| !seen_cache.peer_has_seen(h, peer_id))
        .take(MAX_SYNC_TXS)
        .copied()
        .collect();

    NewPooledTxHashes {
        types:  vec![2u8; hashes.len()], // assume EIP-1559
        sizes:  vec![200u32; hashes.len()], // estimate
        hashes,
    }
}

// ── Max peers per announce ────────────────────────────────────────────────────

/// Maximum number of peers to announce a new tx to directly (full body).
/// Remaining peers receive hash-only announcement (EIP-2464).
///
/// Empirical: sqrt(peer_count) is a good balance between propagation speed
/// and bandwidth efficiency.
pub const MAX_DIRECT_BROADCAST_PEERS: usize = 8;

/// Select which peers get the full tx body vs hash-only announcement.
pub fn select_broadcast_peers(
    all_peers:   &[String],
    hash:        &[u8; 32],
    seen_cache:  &SeenTxCache,
) -> (Vec<String>, Vec<String>) {
    let eligible: Vec<&String> = all_peers.iter()
        .filter(|p| !seen_cache.peer_has_seen(hash, p))
        .collect();

    let split = MAX_DIRECT_BROADCAST_PEERS.min(eligible.len());
    let full_body = eligible[..split].iter().map(|s| s.to_string()).collect();
    let hash_only = eligible[split..].iter().map(|s| s.to_string()).collect();
    (full_body, hash_only)
}