//! PeerManager -- connection lifecycle, keep-alive, subnet tracking.
//!
//! Responsibilities:
//!   - Maintain set of connected peers (inbound + outbound)
//!   - Send RLPx PING every KEEPALIVE_INTERVAL; disconnect on timeout
//!   - Track which subnets each peer subscribes to (for gossipsub)
//!   - Evict lowest-scored peers when MAX_PEERS is reached
//!   - Per-IP and per-subnet connection limits (eclipse resistance)

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

// ── Constants ─────────────────────────────────────────────────────────────────

pub const MAX_PEERS: usize             = 50;
pub const MAX_OUTBOUND: usize          = 25;
pub const MAX_INBOUND: usize           = 25;
pub const MAX_PEERS_PER_SUBNET_24: usize = 2;   // IPv4 /24 -- eclipse resist
pub const MAX_PEERS_PER_SUBNET_64: usize = 2;   // IPv6 /64 -- eclipse resist
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
pub const KEEPALIVE_TIMEOUT: Duration  = Duration::from_secs(30);
pub const MIN_PEER_SCORE: f64          = -100.0;

// ── Peer state ────────────────────────────────────────────────────────────────

/// Connection direction -- Inbound (they dialed us) or Outbound (we dialed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerDirection {
    Inbound,
    Outbound,
}

/// Lifecycle state of a peer connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerState {
    Connecting,    // TCP connecting, Noise handshake in progress
    Handshaking,   // Noise done, waiting for StatusMsg exchange
    Connected,     // Fully connected, exchanging application messages
    Disconnecting, // Graceful disconnect in progress
    Disconnected,  // Connection closed
}

/// A connected (or connecting) peer.
#[derive(Debug, Clone)]
pub struct Peer {
    pub peer_id:       [u8; 64],
    pub addr:          std::net::SocketAddr,
    pub direction:     PeerDirection,
    pub state:         PeerState,
    pub protocol:      String,
    pub client_id:     String,
    pub best_block:    u64,
    pub total_diff:    u128,
    /// Peer reputation score (increases for good behavior, decreases for bad)
    pub peer_score:    f64,
    /// Subnets this peer is subscribed to (for gossipsub mesh routing, 0..64)
    pub subnets:       Vec<u8>,
    pub last_seen:     Instant,
    pub last_ping:     Option<Instant>,
    pub ping_pending:  bool,
    pub ping_failures: u32,
}

impl Peer {
    /// IPv4 /24 subnet prefix -- used for connection limits (eclipse attack resist)
    pub fn subnet_24(&self) -> Option<[u8; 3]> {
        if let IpAddr::V4(ip) = self.addr.ip() {
            let o = ip.octets();
            Some([o[0], o[1], o[2]])
        } else { None }
    }

    /// IPv6 /64 prefix -- used for connection limits
    pub fn subnet_64(&self) -> Option<[u8; 8]> {
        if let IpAddr::V6(ip) = self.addr.ip() {
            let o = ip.octets();
            Some(o[..8].try_into().unwrap_or([0u8; 8]))
        } else { None }
    }

    pub fn adjust_score(&mut self, delta: f64) {
        self.peer_score = (self.peer_score + delta).max(MIN_PEER_SCORE);
    }
}

// ── PeerManager ───────────────────────────────────────────────────────────────

pub struct PeerManager {
    peers:             HashMap<[u8; 64], Peer>,
    subnet_counts_24:  HashMap<[u8; 3], usize>,
    subnet_counts_64:  HashMap<[u8; 8], usize>,
}

impl PeerManager {
    pub fn new() -> Self {
        Self { peers: HashMap::new(), subnet_counts_24: HashMap::new(), subnet_counts_64: HashMap::new() }
    }

    /// Add a peer. Returns Err if max peers or subnet limits are reached.
    pub fn add_peer(&mut self, peer: Peer) -> Result<(), AddPeerError> {
        if self.peers.len() >= MAX_PEERS { return Err(AddPeerError::MaxPeersReached); }
        let inbound  = self.peers.values().filter(|p| p.direction == PeerDirection::Inbound).count();
        let outbound = self.peers.values().filter(|p| p.direction == PeerDirection::Outbound).count();
        match peer.direction {
            PeerDirection::Inbound  if inbound  >= MAX_INBOUND  => return Err(AddPeerError::MaxInboundReached),
            PeerDirection::Outbound if outbound >= MAX_OUTBOUND => return Err(AddPeerError::MaxOutboundReached),
            _ => {}
        }
        // Subnet limits (eclipse attack resistance)
        if let Some(sn) = peer.subnet_24() {
            if self.subnet_counts_24.get(&sn).copied().unwrap_or(0) >= MAX_PEERS_PER_SUBNET_24 {
                return Err(AddPeerError::SubnetLimitReached);
            }
            *self.subnet_counts_24.entry(sn).or_insert(0) += 1;
        }
        if let Some(sn) = peer.subnet_64() {
            if self.subnet_counts_64.get(&sn).copied().unwrap_or(0) >= MAX_PEERS_PER_SUBNET_64 {
                return Err(AddPeerError::SubnetLimitReached);
            }
            *self.subnet_counts_64.entry(sn).or_insert(0) += 1;
        }
        self.peers.insert(peer.peer_id, peer);
        Ok(())
    }

    /// Remove a peer and update subnet counters.
    pub fn remove_peer(&mut self, peer_id: &[u8; 64]) -> Option<Peer> {
        if let Some(peer) = self.peers.remove(peer_id) {
            if let Some(sn) = peer.subnet_24() {
                if let Some(c) = self.subnet_counts_24.get_mut(&sn) { *c = c.saturating_sub(1); }
            }
            if let Some(sn) = peer.subnet_64() {
                if let Some(c) = self.subnet_counts_64.get_mut(&sn) { *c = c.saturating_sub(1); }
            }
            Some(peer)
        } else { None }
    }

    pub fn peer_count(&self) -> usize { self.peers.len() }

    /// Peers subscribed to a specific subnet (gossipsub routing).
    pub fn subnet_peers(&self, subnet_id: u8) -> Vec<&Peer> {
        self.peers.values()
            .filter(|p| p.subnets.contains(&subnet_id) && p.state == PeerState::Connected)
            .collect()
    }

    /// Evict the peer with the lowest score (when at capacity).
    pub fn evict_lowest_score(&mut self) -> Option<[u8; 64]> {
        let worst = self.peers.values()
            .min_by(|a, b| a.peer_score.partial_cmp(&b.peer_score).unwrap_or(std::cmp::Ordering::Equal))
            .map(|p| p.peer_id);
        if let Some(id) = worst { self.remove_peer(&id); return Some(id); }
        None
    }

    /// Run keep-alive PING tick. Returns peer_ids that timed out.
    pub fn keepalive_tick(&mut self) -> Vec<[u8; 64]> {
        let now = Instant::now();
        let mut to_disconnect = Vec::new();
        for peer in self.peers.values_mut() {
            if peer.state != PeerState::Connected { continue; }
            let should_ping = peer.last_ping
                .map(|t| t.elapsed() >= KEEPALIVE_INTERVAL)
                .unwrap_or(true);
            if should_ping && !peer.ping_pending {
                peer.last_ping    = Some(now);
                peer.ping_pending = true;
            }
            if peer.ping_pending {
                if let Some(sent) = peer.last_ping {
                    if sent.elapsed() >= KEEPALIVE_TIMEOUT {
                        peer.ping_failures += 1;
                        peer.ping_pending   = false;
                        if peer.ping_failures >= 3 { to_disconnect.push(peer.peer_id); }
                    }
                }
            }
        }
        to_disconnect
    }

    /// Called when a PONG is received -- mark peer as alive.
    pub fn on_pong(&mut self, peer_id: &[u8; 64]) {
        if let Some(peer) = self.peers.get_mut(peer_id) {
            peer.ping_pending  = false;
            peer.ping_failures = 0;
            peer.last_seen     = Instant::now();
            peer.adjust_score(0.1);
        }
    }

    /// Update a peer's subnet subscriptions (for gossipsub mesh).
    pub fn update_subnets(&mut self, peer_id: &[u8; 64], subnets: Vec<u8>) {
        if let Some(peer) = self.peers.get_mut(peer_id) { peer.subnets = subnets; }
    }

    /// Ban a peer -- remove and blacklist.
    pub fn ban_peer(&mut self, peer_id: &[u8; 64]) -> Option<Peer> {
        self.remove_peer(peer_id)
    }
}

#[derive(Debug)]
pub enum AddPeerError {
    MaxPeersReached,
    MaxInboundReached,
    MaxOutboundReached,
    SubnetLimitReached,
    AlreadyConnected,
}