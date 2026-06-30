//! Cross-chain packet definition and commitment computation.
//!
//! A packet is the atomic unit of cross-chain communication.
//! Its lifecycle: Pending → Committed (src) → Received (dst) → Acknowledged (src).
//! On timeout:   Pending → Committed (src) → Timeout (src, refunded).
//!
//! ## Commitment scheme
//!
//! On the source chain we store:
//!   `commitment_store[channel_id][sequence] = keccak256(canonical_packet_bytes)`
//!
//! The counterparty chain fetches a Merkle proof against the src state trie to
//! verify this commitment without trusting any relayer.

use sha3::{Digest, Keccak256};
use serde::{Deserialize, Serialize};
use zbx_types::H256;

/// Globally unique channel identifier (32 bytes).
pub type ChannelId = [u8; 32];
/// Client identifier — references a foreign-chain light client (32 bytes).
pub type ClientId  = [u8; 32];

/// The cross-chain packet — identical format on both chains.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct XclPacket {
    /// Monotonically increasing per-channel sequence number (1-indexed).
    pub sequence:            u64,
    /// Chain ID of the originating chain.
    pub src_chain_id:        u64,
    /// Chain ID of the destination chain.
    pub dst_chain_id:        u64,
    /// Source channel on the originating chain.
    pub src_channel:         ChannelId,
    /// Counterparty channel on the destination chain.
    pub dst_channel:         ChannelId,
    /// Application-layer payload (e.g. ABI-encoded FtPacketData).
    pub data:                Vec<u8>,
    /// Absolute block height on the **destination** chain at which this
    /// packet expires. 0 = no height timeout.
    pub timeout_height:      u64,
    /// Unix timestamp (seconds) at which this packet expires.
    /// 0 = no timestamp timeout.
    pub timeout_timestamp:   u64,
}

/// Packet status as seen by each chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PacketStatus {
    /// Commitment written on src, waiting for relayer.
    Committed,
    /// Receipt written on dst; funds released.
    Received,
    /// Ack written on dst; src has been notified.
    Acknowledged,
    /// Timed out; src refund complete.
    TimedOut,
}

/// Acknowledgement written by the receiver into its state trie.
/// A successful ack carries `result = 0x01`; error acks carry an error code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PacketAck {
    /// `0x01` for success; any other byte signals an application error.
    pub result: u8,
    /// Optional error string (non-empty only when `result != 0x01`).
    pub error:  String,
}

impl PacketAck {
    pub fn success() -> Self {
        PacketAck { result: 0x01, error: String::new() }
    }

    pub fn error(msg: &str) -> Self {
        PacketAck { result: 0x00, error: msg.to_string() }
    }

    pub fn is_success(&self) -> bool {
        self.result == 0x01
    }

    /// Canonical encoding for commitment / proof purposes.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.push(self.result);
        let err_bytes = self.error.as_bytes();
        out.extend_from_slice(&(err_bytes.len() as u32).to_be_bytes());
        out.extend_from_slice(err_bytes);
        out
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.is_empty() {
            return None;
        }
        let result = bytes[0];
        if bytes.len() < 5 {
            return Some(PacketAck { result, error: String::new() });
        }
        let len = u32::from_be_bytes(bytes[1..5].try_into().ok()?) as usize;
        if bytes.len() < 5 + len {
            return None;
        }
        let error = String::from_utf8(bytes[5..5 + len].to_vec()).ok()?;
        Some(PacketAck { result, error })
    }
}

impl XclPacket {
    /// Canonical deterministic encoding for commitment hashing.
    ///
    /// Format (all big-endian):
    ///   sequence(8) || src_chain(8) || dst_chain(8) ||
    ///   src_channel(32) || dst_channel(32) ||
    ///   timeout_height(8) || timeout_timestamp(8) ||
    ///   data_len(4) || data
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(8 + 8 + 8 + 32 + 32 + 8 + 8 + 4 + self.data.len());
        buf.extend_from_slice(&self.sequence.to_be_bytes());
        buf.extend_from_slice(&self.src_chain_id.to_be_bytes());
        buf.extend_from_slice(&self.dst_chain_id.to_be_bytes());
        buf.extend_from_slice(&self.src_channel);
        buf.extend_from_slice(&self.dst_channel);
        buf.extend_from_slice(&self.timeout_height.to_be_bytes());
        buf.extend_from_slice(&self.timeout_timestamp.to_be_bytes());
        buf.extend_from_slice(&(self.data.len() as u32).to_be_bytes());
        buf.extend_from_slice(&self.data);
        buf
    }

    /// Packet commitment: keccak256 of canonical bytes.
    /// This value is stored on the source chain and proven to the destination.
    pub fn commitment(&self) -> H256 {
        let bytes = self.canonical_bytes();
        let hash = Keccak256::digest(&bytes);
        let mut out = [0u8; 32];
        out.copy_from_slice(&hash);
        H256(out)
    }

    /// Returns `true` if this packet has expired at `current_height`.
    pub fn is_timed_out_height(&self, current_height: u64) -> bool {
        self.timeout_height > 0 && current_height >= self.timeout_height
    }

    /// Returns `true` if this packet has expired at `current_timestamp`.
    pub fn is_timed_out_timestamp(&self, current_ts: u64) -> bool {
        self.timeout_timestamp > 0 && current_ts >= self.timeout_timestamp
    }
}

/// Deterministic state trie key for a packet commitment on the source chain.
///
/// `xcl/commitment/{channel_hex}/{sequence_be8}`
pub fn commitment_key(channel: &ChannelId, sequence: u64) -> Vec<u8> {
    let mut key = b"xcl/commitment/".to_vec();
    key.extend_from_slice(hex::encode(channel).as_bytes());
    key.push(b'/');
    key.extend_from_slice(&sequence.to_be_bytes());
    key
}

/// Deterministic state trie key for a packet receipt on the destination chain.
///
/// `xcl/receipt/{channel_hex}/{sequence_be8}`
pub fn receipt_key(channel: &ChannelId, sequence: u64) -> Vec<u8> {
    let mut key = b"xcl/receipt/".to_vec();
    key.extend_from_slice(hex::encode(channel).as_bytes());
    key.push(b'/');
    key.extend_from_slice(&sequence.to_be_bytes());
    key
}

/// Deterministic state trie key for a packet acknowledgement on the destination chain.
///
/// `xcl/ack/{channel_hex}/{sequence_be8}`
pub fn ack_key(channel: &ChannelId, sequence: u64) -> Vec<u8> {
    let mut key = b"xcl/ack/".to_vec();
    key.extend_from_slice(hex::encode(channel).as_bytes());
    key.push(b'/');
    key.extend_from_slice(&sequence.to_be_bytes());
    key
}
