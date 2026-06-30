//! Network message types for the Zebvix P2P protocol.
//!
//! ## Message ranges
//!
//! | Range | Category |
//! |-------|----------|
//! | 0x01–0x09 | Session / handshake |
//! | 0x10–0x1F | Block sync |
//! | 0x20–0x2F | Transaction relay |
//! | 0x30–0x3F | Consensus (HotStuff v1 + HotStuff-2 / ZEP-022) |
//! | 0x40–0x4F | Gossip |

use zbx_types::{block::{Block, BlockHeader}, transaction::SignedTransaction, H256};
use zbx_consensus::{
    vote::{Vote, QuorumCertificate},
    hotstuff2::{TimeoutShare, TimeoutCertificate},
};
use serde::{Deserialize, Serialize};

/// Discriminant for all P2P message types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum MessageType {
    // ── Session / handshake ───────────────────────────────────────────────────
    Ping           = 0x01,
    Pong           = 0x02,
    Status         = 0x03,
    FindPeers      = 0x04,
    Peers          = 0x05,
    // ── Block sync ────────────────────────────────────────────────────────────
    GetBlockByHash = 0x10,
    Block          = 0x11,
    GetBlockRange  = 0x12,
    Blocks         = 0x13,
    // ── Fast-sync (SEC-2026-05-09 Pass-11) ───────────────────────────────────
    GetHeaders         = 0x14,
    Headers            = 0x15,
    GetSnapshotMeta    = 0x16,
    SnapshotMetaResp   = 0x17,
    GetSnapshotChunk   = 0x18,
    SnapshotChunkResp  = 0x19,
    // ── Transaction relay ─────────────────────────────────────────────────────
    Transaction    = 0x20,
    Transactions   = 0x21,
    // ── Consensus: HotStuff v1 ────────────────────────────────────────────────
    Vote           = 0x30,
    QuorumCert     = 0x31,
    Proposal       = 0x32,
    Timeout        = 0x33,
    // ── Consensus: HotStuff-2 / ZEP-022 ──────────────────────────────────────
    /// A HotStuff-2 block proposal — carries the block, QC justify, and
    /// an optional Timeout Certificate for the Jolteon view-change case.
    Hs2Proposal    = 0x34,
    /// A Jolteon timeout share — broadcast when a validator's round timer
    /// expires. 2f+1 shares form a Timeout Certificate.
    TimeoutShare   = 0x35,
    /// A formed Timeout Certificate — broadcast by the new leader after
    /// collecting 2f+1 timeout shares (triggers view change).
    TimeoutCert    = 0x36,
    // ── Gossip relay ──────────────────────────────────────────────────────────
    /// Gossip relay envelope (wraps any topic payload).
    Gossip         = 0x40,
}

/// Status message exchanged during handshake.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusMessage {
    pub protocol_version: u8,
    pub chain_id: u64,
    pub genesis_hash: H256,
    pub best_block_hash: H256,
    pub best_block_number: u64,
    pub node_pubkey: Vec<u8>, // 65-byte secp256k1 uncompressed
}

/// Block sync request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetBlockRange {
    pub from: u64,
    pub to: u64,   // inclusive, max 64 blocks per request
}

// ── HotStuff-2 message payloads (ZEP-022) ─────────────────────────────────────

/// A HotStuff-2 block proposal message (MessageType::Hs2Proposal = 0x34).
///
/// The block carries:
/// - `block`: the proposed block at round `r`.
/// - `qc`:    the QC justify (certifies the parent block at round `r-1`).
/// - `tc`:    optional Timeout Certificate for Jolteon view-change; present
///            when the leader was elected after a round timeout rather than
///            after a normal QC from the previous round.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hs2ProposalMessage {
    pub block: Box<Block>,
    pub qc:    QuorumCertificate,
    pub tc:    Option<TimeoutCertificate>,
}

// ── Fast-sync payloads (SEC-2026-05-09 Pass-11) ─────────────────────────────

/// Request a contiguous range of block headers.
///
/// Headers-first sync (Bitcoin / geth eth/63 style): the syncer
/// downloads + verifies the header chain to a recent finalized
/// pivot, then snap-syncs the state at that pivot, then live-
/// catches-up bodies. `count` is bounded by `MAX_HEADERS_PER_RESP`
/// at the responder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetHeadersMessage {
    pub from:    u64,
    pub count:   u32,
    pub reverse: bool,
}

/// Per-snapshot manifest. The `state_root` is the post-state root
/// of the pivot block; `chunk_roots[i]` is the root of the i-th
/// self-contained mini-trie in the snapshot. The responder commits
/// to this list with `manifest_hash = keccak256(rlp(chunk_roots))`,
/// which the requester checks against the pivot block's header
/// (Pass-12 will hard-bind via a header field; today the requester
/// trusts the pivot block + verifies each chunk against the
/// committed `chunk_roots[i]`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotMeta {
    pub pivot_height: u64,
    pub state_root:   H256,
    pub total_chunks: u64,
    pub chunk_roots:  Vec<H256>,
}

/// Request a single chunk of the snapshot at `pivot_height`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetSnapshotChunkMessage {
    pub pivot_height: u64,
    pub chunk_id:     u64,
}

/// A snapshot chunk response: the leaves of one mini-trie. The
/// requester rebuilds a `MutableTrie` from these leaves and checks
/// the computed root equals `meta.chunk_roots[chunk_id]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotChunkResponse {
    pub pivot_height: u64,
    pub chunk_id:     u64,
    pub leaves:       Vec<(H256, Vec<u8>)>,
}

/// A wrapper for gossip relay messages (MessageType range 0x40–0x4F).
///
/// The `topic` byte identifies the gossip topic (mirrors `GossipTopic::as u8`).
/// The `payload` is the inner serialised message (block, tx, vote, etc.).
/// The `message_id` is `keccak256(topic || payload)` for deduplication.
/// The `ttl` is the remaining hop count (decremented on each relay).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GossipEnvelope {
    pub topic:      u8,
    pub payload:    Vec<u8>,
    pub message_id: H256,
    pub ttl:        u8,
}

/// The main message enum.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Message {
    // ── Session / handshake ───────────────────────────────────────────────────
    Ping { nonce: u64 },
    Pong { nonce: u64 },
    Status(StatusMessage),
    FindPeers { target: H256 },
    Peers(Vec<String>), // multiaddrs
    // ── Block sync ────────────────────────────────────────────────────────────
    GetBlockByHash(H256),
    Block(Box<Block>),
    GetBlockRange(GetBlockRange),
    Blocks(Vec<Box<Block>>),
    // ── Fast-sync (SEC-2026-05-09 Pass-11) ───────────────────────────────────
    GetHeaders(GetHeadersMessage),
    Headers(Vec<BlockHeader>),
    GetSnapshotMeta { pivot_height: u64 },
    SnapshotMetaResp(SnapshotMeta),
    GetSnapshotChunk(GetSnapshotChunkMessage),
    SnapshotChunkResp(SnapshotChunkResponse),
    // ── Transaction relay ─────────────────────────────────────────────────────
    Transaction(SignedTransaction),
    Transactions(Vec<SignedTransaction>),
    // ── Consensus: HotStuff v1 ────────────────────────────────────────────────
    Vote(Vote),
    QuorumCert(QuorumCertificate),
    Proposal { block: Box<Block>, qc: QuorumCertificate },
    Timeout { round: u64, epoch: u64 },
    // ── Consensus: HotStuff-2 / ZEP-022 ──────────────────────────────────────
    /// HotStuff-2 proposal (block + QC justify + optional TC for view-change).
    Hs2Proposal(Hs2ProposalMessage),
    /// A Jolteon timeout share from one validator.
    TimeoutShareMsg(TimeoutShare),
    /// A formed Timeout Certificate (2f+1 timeout shares).
    TimeoutCertMsg(TimeoutCertificate),
    // ── Gossip relay ──────────────────────────────────────────────────────────
    GossipMsg(GossipEnvelope),
}

impl Message {
    pub fn message_type(&self) -> MessageType {
        match self {
            Message::Ping { .. }          => MessageType::Ping,
            Message::Pong { .. }          => MessageType::Pong,
            Message::Status(_)            => MessageType::Status,
            Message::FindPeers { .. }     => MessageType::FindPeers,
            Message::Peers(_)             => MessageType::Peers,
            Message::GetBlockByHash(_)    => MessageType::GetBlockByHash,
            Message::Block(_)             => MessageType::Block,
            Message::GetBlockRange(_)     => MessageType::GetBlockRange,
            Message::Blocks(_)            => MessageType::Blocks,
            Message::GetHeaders(_)        => MessageType::GetHeaders,
            Message::Headers(_)           => MessageType::Headers,
            Message::GetSnapshotMeta { .. } => MessageType::GetSnapshotMeta,
            Message::SnapshotMetaResp(_)  => MessageType::SnapshotMetaResp,
            Message::GetSnapshotChunk(_)  => MessageType::GetSnapshotChunk,
            Message::SnapshotChunkResp(_) => MessageType::SnapshotChunkResp,
            Message::Transaction(_)       => MessageType::Transaction,
            Message::Transactions(_)      => MessageType::Transactions,
            Message::Vote(_)              => MessageType::Vote,
            Message::QuorumCert(_)        => MessageType::QuorumCert,
            Message::Proposal { .. }      => MessageType::Proposal,
            Message::Timeout { .. }       => MessageType::Timeout,
            Message::Hs2Proposal(_)       => MessageType::Hs2Proposal,
            Message::TimeoutShareMsg(_)   => MessageType::TimeoutShare,
            Message::TimeoutCertMsg(_)    => MessageType::TimeoutCert,
            Message::GossipMsg(_)         => MessageType::Gossip,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, crate::NetworkError> {
        serde_json::from_slice(bytes)
            .map_err(|e| crate::NetworkError::MessageDecode(e.to_string()))
    }
}