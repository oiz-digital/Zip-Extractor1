//! Peer lifecycle management -- connect, disconnect, reputation, discovery.
//!
//! The PeerManager is the central component for network peer handling:
//!   - Tracks all connected peers (inbound + outbound)
//!   - Manages peer reputation scores (good behavior = +score, bad = -score)
//!   - Bans peers that reach minimum reputation
//!   - Discovers new peers via bootnodes / Kademlia DHT / discv5
//!   - Enforces max_peers limits
//!
//! ## Peer reputation
//!   Each peer starts at score 0 (neutral).
//!   Score range: -100 (ban threshold) to +100 (excellent)
//!   Score changes:
//!     +5  : valid block/tx relayed
//!     +2  : valid response to request
//!     -10 : invalid message / bad response
//!     -20 : protocol violation
//!     -50 : equivocation / double-sign evidence
//!     -100: immediate ban (on detection of malicious behavior)
//!
//! ## Peer discovery
//!   1. Bootnodes: hardcoded list of trusted bootstrap nodes
//!   2. discv5: UDP-based peer discovery (same as Ethereum beacon chain)
//!   3. Kademlia: maintain routing table, find_node() RPC
//!   4. PEX (Peer Exchange): connected peers share their known peers
//!
//! ## Inbound / outbound limits
//!   MAX_PEERS = 50 total
//!   MAX_OUTBOUND_PEERS = 30 (we initiate connection)
//!   MAX_INBOUND_PEERS  = 20 (they initiate connection)

use std::collections::HashMap;
use std::time::Duration;

// ── Constants ─────────────────────────────────────────────────────────────────

pub const MAX_PEERS:          usize = 50;
pub const MAX_OUTBOUND_PEERS: usize = 30;
pub const MAX_INBOUND_PEERS:  usize = 20;
pub const MIN_OUTBOUND_PEERS: usize = 8;   // try to maintain at least 8 outbound
pub const BAN_THRESHOLD:      i32   = -100;
pub const BAN_DURATION_SECS:  u64   = 3_600; // 1 hour ban

// ── Peer state ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerDirection {
    Inbound,   // They connected to us
    Outbound,  // We connected to them
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerStatus {
    Connecting,
    Connected,
    Disconnecting,
    Banned { until: u64 },
}

/// Full peer state for a connected peer.
#[derive(Debug, Clone)]
pub struct PeerState {
    pub peer_id:         String,          // libp2p PeerId
    pub addr:            String,          // Multiaddr
    pub direction:       PeerDirection,
    pub status:          PeerStatus,
    pub reputation:      i32,             // -100 to +100
    pub connected_at:    u64,             // Unix timestamp
    pub last_seen:       u64,
    pub latency_ms:      Option<u64>,
    pub protocol_version: u32,
    pub best_block:      u64,             // Peer's claimed best block
    pub best_hash:       [u8; 32],
    pub chain_id:        u64,
    pub bytes_sent:      u64,
    pub bytes_recv:      u64,
}

// ── Peer reputation ───────────────────────────────────────────────────────────

/// Reputation change events (for audit trail).
#[derive(Debug, Clone)]
pub enum ReputationEvent {
    ValidBlock,          // +5
    ValidTx,             // +5
    ValidResponse,       // +2
    InvalidMessage,      // -10
    ProtocolViolation,   // -20
    SlowResponse,        // -5
    Equivocation,        // -50
    ImmediateBan,        // -100
}

impl ReputationEvent {
    pub fn score_delta(&self) -> i32 {
        match self {
            Self::ValidBlock       =>  5,
            Self::ValidTx         =>  5,
            Self::ValidResponse   =>  2,
            Self::InvalidMessage  => -10,
            Self::ProtocolViolation => -20,
            Self::SlowResponse    =>  -5,
            Self::Equivocation    => -50,
            Self::ImmediateBan    => -100,
        }
    }
}

// ── Peer discovery ────────────────────────────────────────────────────────────

/// ZBX mainnet bootnodes (hardcoded bootstrap peers).
pub const BOOTNODES: &[&str] = &[
    "enr:-Iq4QPh-LZGP7JrHp...zbx-bootnode-1.zebvix.io",
    "enr:-Iq4QK9mD7zP1LrAq...zbx-bootnode-2.zebvix.io",
    "enr:-Iq4QM1xRYaB3HLqP...zbx-bootnode-3.zebvix.io",
];

/// ZBX testnet bootnodes.
pub const TESTNET_BOOTNODES: &[&str] = &[
    "enr:-Iq4QTestBootnd1...zbx-testnet-boot-1.zebvix.io",
];

/// Peer discovery state and known peer candidates.
pub struct PeerDiscovery {
    /// Candidates discovered but not yet connected
    pub candidates:  Vec<PeerCandidate>,
    /// Bootnodes (always try to connect on startup)
    pub bootnodes:   Vec<String>,
    /// Peers discovered via Kademlia routing table
    pub kademlia_peers: Vec<PeerCandidate>,
    /// Peers received via PEX (peer exchange) from connected peers
    pub pex_peers:   Vec<PeerCandidate>,
    /// Last discovery attempt timestamp
    pub last_discover: u64,
    /// Discovery interval (try to find more peers every N seconds)
    pub discover_interval: u64,
}

#[derive(Debug, Clone)]
pub struct PeerCandidate {
    pub peer_id:  String,
    pub addr:     String,
    pub source:   DiscoverySource,
    pub score:    i32,  // Estimated quality before connecting
}

#[derive(Debug, Clone)]
pub enum DiscoverySource {
    Bootnode,
    Kademlia,
    Discv5,
    PeerExchange,
    Manual,
}

impl PeerDiscovery {
    pub fn new(bootnodes: Vec<String>) -> Self {
        let candidates: Vec<PeerCandidate> = bootnodes.iter().map(|addr| PeerCandidate {
            peer_id: String::new(),
            addr:    addr.clone(),
            source:  DiscoverySource::Bootnode,
            score:   50, // bootnodes are trusted, start with positive score
        }).collect();
        Self {
            candidates,
            bootnodes,
            kademlia_peers: Vec::new(),
            pex_peers:      Vec::new(),
            last_discover:  0,
            discover_interval: 30, // re-discover every 30 seconds
        }
    }

    /// Add a peer candidate discovered via any source.
    pub fn add_candidate(&mut self, candidate: PeerCandidate) {
        if !self.candidates.iter().any(|c| c.addr == candidate.addr) {
            self.candidates.push(candidate);
        }
    }

    /// Get next candidates to try connecting to (best score first).
    pub fn next_candidates(&mut self, count: usize) -> Vec<PeerCandidate> {
        self.candidates.sort_by(|a, b| b.score.cmp(&a.score));
        self.candidates.drain(..count.min(self.candidates.len())).collect()
    }
}

// ── Peer manager ──────────────────────────────────────────────────────────────

/// Central peer manager -- handles all peer lifecycle events.
pub struct PeerManager {
    pub peers:          HashMap<String, PeerState>,  // peer_id -> state
    pub banned:         HashMap<String, u64>,         // peer_id -> ban_until
    pub discovery:      PeerDiscovery,
    pub reputation_log: Vec<(String, ReputationEvent, u64)>, // audit log
}

impl PeerManager {
    pub fn new(bootnodes: Vec<String>) -> Self {
        Self {
            peers:          HashMap::new(),
            banned:         HashMap::new(),
            discovery:      PeerDiscovery::new(bootnodes),
            reputation_log: Vec::new(),
        }
    }

    /// Called when a new peer connects (inbound or outbound).
    pub fn on_peer_connected(&mut self, peer: PeerState) -> Result<(), PeerError> {
        // Check ban list
        if self.is_banned(&peer.peer_id) { return Err(PeerError::Banned); }
        // Enforce connection limits
        let (inbound, outbound) = self.count_by_direction();
        match peer.direction {
            PeerDirection::Inbound  if inbound  >= MAX_INBOUND_PEERS  => return Err(PeerError::TooManyInbound),
            PeerDirection::Outbound if outbound >= MAX_OUTBOUND_PEERS => return Err(PeerError::TooManyOutbound),
            _ => {}
        }
        if self.peers.len() >= MAX_PEERS { return Err(PeerError::MaxPeersReached); }
        self.peers.insert(peer.peer_id.clone(), peer);
        Ok(())
    }

    /// Called when a peer disconnects.
    pub fn on_peer_disconnected(&mut self, peer_id: &str, reason: DisconnectReason) {
        self.peers.remove(peer_id);
        // Apply reputation penalty for unexpected disconnects
        if matches!(reason, DisconnectReason::ProtocolError | DisconnectReason::Timeout) {
            // Add back as candidate with lower score for potential reconnect
        }
    }

    /// Apply a reputation event to a peer.
    pub fn apply_reputation(&mut self, peer_id: &str, event: ReputationEvent, now: u64) {
        let delta = event.score_delta();
        self.reputation_log.push((peer_id.to_string(), event, now));
        if let Some(peer) = self.peers.get_mut(peer_id) {
            peer.reputation = (peer.reputation + delta).clamp(-100, 100);
            if peer.reputation <= BAN_THRESHOLD {
                self.ban_peer(peer_id, now + BAN_DURATION_SECS);
            }
        }
    }

    /// Ban a peer until a given timestamp.
    pub fn ban_peer(&mut self, peer_id: &str, until: u64) {
        self.banned.insert(peer_id.to_string(), until);
        self.peers.remove(peer_id);
    }

    pub fn is_banned(&self, peer_id: &str) -> bool {
        self.banned.contains_key(peer_id)
    }

    pub fn count_by_direction(&self) -> (usize, usize) {
        let inbound  = self.peers.values().filter(|p| p.direction == PeerDirection::Inbound).count();
        let outbound = self.peers.values().filter(|p| p.direction == PeerDirection::Outbound).count();
        (inbound, outbound)
    }

    pub fn peer_count(&self) -> usize { self.peers.len() }
    pub fn needs_more_peers(&self) -> bool {
        let (_, outbound) = self.count_by_direction();
        outbound < MIN_OUTBOUND_PEERS
    }
}

#[derive(Debug)]
pub enum PeerError {
    Banned, MaxPeersReached, TooManyInbound, TooManyOutbound, AlreadyConnected,
}

#[derive(Debug)]
pub enum DisconnectReason {
    Graceful, ProtocolError, Timeout, Banned, TooManyPeers, UselessPeer,
}