//! discv5 -- Peer Discovery Protocol v5
//!
//! ZBX uses discv5 over UDP to discover peers.
//! Reference: <https://github.com/ethereum/devp2p/blob/master/discv5/>
//!
//! Packet types
//! 0x01 PING  -- Liveness check, update ENR seq
//! 0x02 PONG  -- Reply to PING, carry external IP
//! 0x03 FINDNODE -- Request k nearest peers to target
//! 0x04 NODES -- Return up to 16 ENR records
//! 0x05 TALKREQ  -- Application-layer extension
//! 0x06 TALKRESP -- Reply to TALKREQ
//!
//! NodeId = keccak256(secp256k1_pubkey_uncompressed[1..])  // 32 bytes
//! Kademlia: K-bucket size k=16, ALPHA=3 concurrent lookups

use std::net::{SocketAddr, Ipv4Addr, Ipv6Addr};
use std::time::{Duration, Instant};
use std::collections::HashMap;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum UDP packet size for discv5 (1280 bytes per spec)
pub const MAX_PACKET_SIZE: usize = 1280;

/// k-bucket size (max peers per bucket)
pub const K_BUCKET_SIZE: usize = 16;

/// Lookup parallelism -- concurrent FINDNODE requests per lookup
pub const ALPHA: usize = 3;

/// PING interval: send a PING to each peer every 30 seconds
pub const PING_INTERVAL: Duration = Duration::from_secs(30);

/// PONG timeout: if no PONG received within 5s, mark peer as unreachable
pub const PONG_TIMEOUT: Duration = Duration::from_secs(5);

/// Max FINDNODE hops before declaring lookup complete
pub const MAX_LOOKUP_HOPS: usize = 8;

/// discv5 UDP port (default)
pub const DEFAULT_DISCV5_PORT: u16 = 30303;

// ── Packet types ──────────────────────────────────────────────────────────────

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketType {
    Ping     = 0x01,
    Pong     = 0x02,
    FindNode = 0x03,
    Nodes    = 0x04,
    TalkReq  = 0x05,
    TalkResp = 0x06,
}

// ── NodeId ────────────────────────────────────────────────────────────────────

/// 256-bit node identifier = keccak256(secp256k1_pubkey_uncompressed[1..])
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct NodeId(pub [u8; 32]);

impl NodeId {
    /// XOR distance between two NodeIds (Kademlia metric).
    pub fn xor_distance(&self, other: &NodeId) -> [u8; 32] {
        let mut d = [0u8; 32];
        for i in 0..32 { d[i] = self.0[i] ^ other.0[i]; }
        d
    }

    /// K-bucket index: highest set bit position in XOR distance.
    pub fn bucket_index(&self, other: &NodeId) -> usize {
        let dist = self.xor_distance(other);
        for i in 0..32 {
            if dist[i] != 0 {
                return (31 - i) * 8 + (7 - dist[i].leading_zeros() as usize);
            }
        }
        0
    }
}

// ── PING ──────────────────────────────────────────────────────────────────────

/// PING request -- liveness check.
/// Carries local ENR sequence number so peers can detect updates.
///
/// Flow:
///   Sender -> PING -> Recipient
///   Recipient -> PONG -> Sender  (with sender's external IP/port)
///
/// On receipt of PING:
///   1. Update last-seen timestamp for the sender in k-buckets
///   2. If sender's ENR seq > our stored seq, send FINDNODE for their new ENR
///   3. Reply with PONG containing our view of sender's IP/port
#[derive(Debug, Clone)]
pub struct Ping {
    /// Request ID (8 random bytes) -- echoed in PONG for correlation
    pub request_id: [u8; 8],
    /// Our current ENR sequence number (monotonically increasing)
    pub enr_seq:    u64,
}

impl Ping {
    pub fn new(enr_seq: u64) -> Self {
        let mut rid = [0u8; 8];
        getrandom::getrandom(&mut rid).unwrap_or_default();
        Self { request_id: rid, enr_seq }
    }
}

// ── PONG ──────────────────────────────────────────────────────────────────────

/// PONG response -- reply to PING.
/// Contains the sender's externally visible IP so they can detect NAT.
///
/// On receipt of PONG:
///   1. Match request_id to outstanding PING
///   2. Update reciprocal_ip -- this is our external NAT-mapped IP
///   3. Update ENR if recipient_enr_seq differs from what we stored
///   4. Mark peer as live in k-bucket
#[derive(Debug, Clone)]
pub struct Pong {
    /// Echo of the request_id from PING
    pub request_id:      [u8; 8],
    /// Our current ENR sequence number
    pub enr_seq:         u64,
    /// The sender's IP as seen by us -- helps with NAT detection
    pub recipient_ip:    RecipientIp,
    /// The sender's UDP port as seen by us
    pub recipient_port:  u16,
}

/// IP address as seen from the network (for NAT traversal).
#[derive(Debug, Clone)]
pub enum RecipientIp {
    V4(Ipv4Addr),
    V6(Ipv6Addr),
}

// ── FINDNODE / NODES ──────────────────────────────────────────────────────────

/// FINDNODE request -- ask a peer for their k nearest peers.
/// Uses "distances" field (list of log2 bucket distances) per discv5 v2.
#[derive(Debug, Clone)]
pub struct FindNode {
    pub request_id: [u8; 8],
    /// List of log2 distances to query (0..256)
    pub distances:  Vec<u8>,
}

/// NODES response -- return ENR records at requested distances.
/// May be split into multiple NODES packets (total field signals how many).
#[derive(Debug, Clone)]
pub struct NodesResponse {
    pub request_id: [u8; 8],
    /// Total number of NODES packets in this response (for reassembly)
    pub total:   u8,
    /// ENR records (max K_BUCKET_SIZE = 16 per packet)
    pub enrs:    Vec<Enr>,
}

// ── ENR (Ethereum Node Record) ────────────────────────────────────────────────

/// Ethereum Node Record -- signed, versioned peer identity.
/// Format: RLP-encoded (seq, k-v pairs) signed by secp256k1 key.
///
/// Fields present in ZBX ENR:
///   id       -> "v4"
///   secp256k1 -> compressed pubkey (33 bytes)
///   ip       -> IPv4 (4 bytes)
///   ip6      -> IPv6 (16 bytes)
///   tcp      -> RLPx TCP port
///   udp      -> discv5 UDP port
#[derive(Debug, Clone)]
pub struct Enr {
    pub seq:       u64,
    pub pubkey:    [u8; 33],
    pub ip:        Option<Ipv4Addr>,
    pub ip6:       Option<Ipv6Addr>,
    pub tcp:       Option<u16>,
    pub udp:       Option<u16>,
    pub signature: [u8; 64],
}

impl Enr {
    /// Derive NodeId = keccak256(uncompressed_pubkey[1..])
    pub fn node_id(&self) -> NodeId {
        let uncompressed = secp256k1_decompress(&self.pubkey);
        let hash = keccak256(&uncompressed[1..]);
        NodeId(hash)
    }

    /// Verify the ENR signature.
    pub fn verify(&self) -> bool {
        let content = rlp_encode_enr_content(self.seq, &self.pubkey, self.ip, self.udp, self.tcp);
        secp256k1_verify(&content, &self.signature, &self.pubkey)
    }

    /// Return the best socket address (prefer IPv4, fallback IPv6)
    pub fn socket_addr_udp(&self) -> Option<SocketAddr> {
        if let (Some(ip), Some(udp)) = (self.ip, self.udp) {
            return Some(SocketAddr::new(std::net::IpAddr::V4(ip), udp));
        }
        if let (Some(ip6), Some(udp)) = (self.ip6, self.udp) {
            return Some(SocketAddr::new(std::net::IpAddr::V6(ip6), udp));
        }
        None
    }
}

// ── Recursive lookup_node ─────────────────────────────────────────────────────

/// Result of a recursive Kademlia lookup.
pub struct LookupResult {
    pub target:       NodeId,
    pub closest_seen: Vec<NodeId>,
    pub hops:         usize,
    pub duration_ms:  u64,
}

/// Recursive Kademlia lookup for a target NodeId.
///
/// Algorithm:
///   1. Start with ALPHA closest known peers from local k-buckets
///   2. Send concurrent FINDNODE requests (ALPHA = 3 parallel)
///   3. Collect NODES responses; add new peers to candidate set
///   4. Sort candidates by XOR distance; take closest K_BUCKET_SIZE
///   5. Repeat with the next unseen closest peers until no progress
///   6. Return up to K_BUCKET_SIZE closest discovered peers
///
/// Used for:
///   - Peer discovery at startup (random target NodeId)
///   - Finding specific nodes (target = that node's NodeId)
///   - k-bucket refresh (target = random ID in bucket range)
pub async fn lookup_node(
    target:        NodeId,
    local_id:      NodeId,
    k_buckets:     &KBuckets,
    send_findnode: impl Fn(NodeId, Vec<u8>) -> std::pin::Pin<Box<dyn std::future::Future<Output=Option<Vec<Enr>>> + Send>>,
) -> LookupResult {
    let start = Instant::now();
    let mut seen:    std::collections::HashSet<NodeId> = std::collections::HashSet::new();
    let mut pending: Vec<NodeId> = k_buckets.closest(&target, ALPHA);
    let mut closest: Vec<NodeId> = Vec::new();
    let mut hops = 0;

    seen.insert(local_id);
    for id in &pending { seen.insert(*id); }

    loop {
        if pending.is_empty() || hops >= MAX_LOOKUP_HOPS { break; }
        hops += 1;

        let batch: Vec<NodeId> = pending.drain(..pending.len().min(ALPHA)).collect();
        let mut futs = Vec::new();
        for peer_id in &batch {
            let distances: Vec<u8> = (250..=255).collect();
            futs.push(send_findnode(*peer_id, distances));
        }
        let results = futures::future::join_all(futs).await;

        for maybe_enrs in results {
            if let Some(enrs) = maybe_enrs {
                for enr in enrs {
                    let id = enr.node_id();
                    if seen.insert(id) {
                        pending.push(id);
                        closest.push(id);
                    }
                }
            }
        }

        closest.sort_by(|a, b| {
            let da = target.xor_distance(a);
            let db = target.xor_distance(b);
            da.cmp(&db)
        });
        closest.truncate(K_BUCKET_SIZE);
    }

    LookupResult {
        target, closest_seen: closest, hops,
        duration_ms: start.elapsed().as_millis() as u64,
    }
}

// ── K-Buckets ─────────────────────────────────────────────────────────────────

/// Kademlia routing table -- 256 k-buckets indexed by log2 XOR distance.
pub struct KBuckets {
    local_id: NodeId,
    buckets:  Vec<Vec<EnrEntry>>,
}

#[derive(Debug, Clone)]
pub struct EnrEntry {
    pub node_id:    NodeId,
    pub enr:        Enr,
    pub last_seen:  Instant,
    pub is_live:    bool,
}

impl KBuckets {
    pub fn new(local_id: NodeId) -> Self {
        Self { local_id, buckets: (0..256).map(|_| Vec::new()).collect() }
    }

    /// Insert or refresh a peer. LRU eviction if bucket is full.
    pub fn insert(&mut self, enr: Enr) {
        let id  = enr.node_id();
        let idx = self.local_id.bucket_index(&id);
        let bucket = &mut self.buckets[idx];
        if let Some(e) = bucket.iter_mut().find(|e| e.node_id == id) {
            e.last_seen = Instant::now();
            e.is_live   = true;
            e.enr       = enr;
            return;
        }
        if bucket.len() < K_BUCKET_SIZE {
            bucket.push(EnrEntry { node_id: id, enr, last_seen: Instant::now(), is_live: true });
        } else if let Some(pos) = bucket.iter().position(|e| !e.is_live) {
            bucket[pos] = EnrEntry { node_id: id, enr, last_seen: Instant::now(), is_live: true };
        }
    }

    /// Return up to n closest known peers sorted by XOR distance.
    pub fn closest(&self, target: &NodeId, n: usize) -> Vec<NodeId> {
        let mut all: Vec<(NodeId, [u8; 32])> = self.buckets
            .iter().flatten()
            .filter(|e| e.is_live)
            .map(|e| (e.node_id, target.xor_distance(&e.node_id)))
            .collect();
        all.sort_by(|a, b| a.1.cmp(&b.1));
        all.iter().take(n).map(|(id, _)| *id).collect()
    }

    pub fn live_peer_count(&self) -> usize {
        self.buckets.iter().flatten().filter(|e| e.is_live).count()
    }

    /// Bootstrap: connect to well-known bootnode ENRs.
    pub async fn bootstrap(&mut self, bootnodes: Vec<Enr>) {
        for enr in bootnodes { self.insert(enr); }
    }
}

// ── PING loop ─────────────────────────────────────────────────────────────────

/// Periodic PING task -- keeps k-bucket entries fresh.
/// Runs every PING_INTERVAL (30s).
/// Marks entries !is_live if PONG not received within PONG_TIMEOUT (5s).
pub async fn run_ping_loop(
    mut k_buckets: KBuckets,
    send_ping: impl Fn(NodeId, Ping) -> std::pin::Pin<Box<dyn std::future::Future<Output=Option<Pong>> + Send>>,
) {
    loop {
        tokio::time::sleep(PING_INTERVAL).await;
        let peers: Vec<NodeId> = k_buckets.buckets.iter().flatten()
            .filter(|e| e.last_seen.elapsed() >= PING_INTERVAL)
            .map(|e| e.node_id).collect();

        for peer_id in peers {
            let ping = Ping::new(0);
            match tokio::time::timeout(PONG_TIMEOUT, send_ping(peer_id, ping)).await {
                Ok(Some(_)) => {
                    for bucket in &mut k_buckets.buckets {
                        if let Some(e) = bucket.iter_mut().find(|e| e.node_id == peer_id) {
                            e.is_live = true; e.last_seen = Instant::now();
                        }
                    }
                }
                _ => {
                    for bucket in &mut k_buckets.buckets {
                        if let Some(e) = bucket.iter_mut().find(|e| e.node_id == peer_id) {
                            e.is_live = false;
                        }
                    }
                }
            }
        }
    }
}

// ── Crypto helpers ────────────────────────────────────────────────────────────

/// H-3 fix: real keccak256 via the secp256k1 crate's sha256 + sha3 (was [0u8; 32]).
fn keccak256(data: &[u8]) -> [u8; 32] {
    // Use secp256k1::hashes which is already a dep, or compute with tiny-keccak.
    // zbx-crypto re-exports keccak256; use it to stay consistent.
    zbx_crypto::keccak::keccak256(data).into()
}

/// H-3 fix: real secp256k1 compressed-point decompression via secp256k1 crate.
///
/// Input:  33-byte compressed SEC1 point (prefix 0x02 or 0x03).
/// Output: 65-byte uncompressed SEC1 point (prefix 0x04 ‖ x ‖ y).
/// Returns [0u8; 65] on invalid input (wrong prefix, point not on curve).
fn secp256k1_decompress(pk: &[u8; 33]) -> [u8; 65] {
    use secp256k1::{PublicKey, Secp256k1};
    let secp = Secp256k1::verification_only();
    match PublicKey::from_slice(pk) {
        Ok(pubkey) => {
            pubkey.serialize_uncompressed()
        }
        Err(e) => {
            tracing::warn!("discv5: secp256k1_decompress: invalid point: {e}");
            [0u8; 65]
        }
    }
}

/// secp256k1 ECDSA verify — used for ENR identity verification.
/// Returns true if `sig` (64-byte compact r‖s) is a valid signature of
/// keccak256(`msg`) under compressed public key `pk`.
fn secp256k1_verify(msg: &[u8], sig: &[u8; 64], pk: &[u8; 33]) -> bool {
    use secp256k1::{ecdsa::Signature, Message, PublicKey, Secp256k1};
    let secp = Secp256k1::verification_only();
    let hash  = keccak256(msg);
    let Ok(msg_obj) = Message::from_digest_slice(&hash) else { return false; };
    let Ok(pubkey)  = PublicKey::from_slice(pk) else { return false; };
    let Ok(sig_obj) = Signature::from_compact(sig) else { return false; };
    secp.verify_ecdsa(&msg_obj, &sig_obj, &pubkey).is_ok()
}

fn rlp_encode_enr_content(_seq: u64, _pk: &[u8; 33], _ip: Option<Ipv4Addr>, _udp: Option<u16>, _tcp: Option<u16>) -> Vec<u8> { vec![] }