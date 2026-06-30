//! Peer management: identity, state, and lifecycle.

use zbx_crypto::secp256k1::PubKey;
use zbx_types::address::Address;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use crate::error::NetworkError;

/// Unique identifier for a network peer — keccak256 of their node public key.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PeerId(pub [u8; 32]);

impl PeerId {
    pub fn from_pubkey(pk: &PubKey) -> Self {
        let h = zbx_crypto::keccak::keccak256(&pk.0);
        PeerId(h.into())
    }

    pub fn short(&self) -> String {
        hex::encode(&self.0[..6])
    }
}

impl std::fmt::Display for PeerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "peer:{}", self.short())
    }
}

/// Connection state of a peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerState {
    Connecting,
    Handshaking,
    Connected,
    Disconnected,
    Banned,
}

/// Full information about a known peer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    pub id: PeerId,
    pub addr: SocketAddr,
    /// EVM address derived from the peer's node key.
    pub node_address: Address,
    pub protocol_version: u8,
    pub chain_id: u64,
    /// Best block number reported by this peer.
    pub best_block: u64,
    /// Round-trip latency (ms).
    pub latency_ms: u32,
    /// Last seen timestamp (seconds since epoch).
    pub last_seen: u64,
}

/// Manages the set of known and connected peers.
pub struct PeerManager {
    pub peers: HashMap<PeerId, PeerInfo>,
    max_peers: usize,
    banned: std::collections::HashSet<std::net::IpAddr>,
}

impl PeerManager {
    pub fn new(max_peers: usize) -> Self {
        PeerManager {
            peers: HashMap::new(),
            max_peers,
            banned: std::collections::HashSet::new(),
        }
    }

    pub fn connected_count(&self) -> usize {
        self.peers.len()
    }

    pub fn is_full(&self) -> bool {
        self.peers.len() >= self.max_peers
    }

    pub fn add_peer(&mut self, info: PeerInfo) -> Result<(), NetworkError> {
        if self.is_full() {
            return Err(NetworkError::MaxPeers(self.max_peers));
        }
        if self.banned.contains(&info.addr.ip()) {
            return Err(NetworkError::ConnectionRefused(
                format!("{} is banned", info.addr.ip())
            ));
        }
        info!(peer = %info.id, addr = %info.addr, "peer added");
        self.peers.insert(info.id.clone(), info);
        Ok(())
    }

    pub fn remove_peer(&mut self, id: &PeerId) {
        if self.peers.remove(id).is_some() {
            warn!(peer = %id, "peer disconnected");
        }
    }

    pub fn ban_peer(&mut self, id: &PeerId, reason: &str) {
        if let Some(info) = self.peers.remove(id) {
            warn!(peer = %id, reason, "peer banned");
            self.banned.insert(info.addr.ip());
        }
    }

    pub fn update_best_block(&mut self, id: &PeerId, best: u64) {
        if let Some(p) = self.peers.get_mut(id) {
            p.best_block = best;
        }
    }

    pub fn update_latency(&mut self, id: &PeerId, latency_ms: u32) {
        if let Some(p) = self.peers.get_mut(id) {
            p.latency_ms = latency_ms;
        }
    }

    /// Peers sorted by best block (for sync target selection).
    pub fn best_peers(&self) -> Vec<&PeerInfo> {
        let mut v: Vec<&PeerInfo> = self.peers.values().collect();
        v.sort_by(|a, b| b.best_block.cmp(&a.best_block));
        v
    }
}