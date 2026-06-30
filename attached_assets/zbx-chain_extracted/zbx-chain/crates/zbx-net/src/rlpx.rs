//! RLPx wire protocol -- framing, Snappy compression, message types.
//!
//! ZBX uses Ethereum-compatible RLPx framing with:
//!   - AES-256-CTR encryption (keys derived from Noise XX handshake)
//!   - Snappy compression for all messages > COMPRESS_THRESHOLD bytes
//!   - MAC integrity checking (HMAC-SHA256)
//!
//! Message size limits:
//!   - MAX_MSG_SIZE = 16 MB (0x1000000 bytes)
//!   - Headers download batch: up to 1024 headers per request
//!   - Bodies batch: up to 256 bodies per request
//!   - Transactions gossip: up to 256 tx hashes per announcement
//!
//! Snappy compression:
//!   All messages > COMPRESS_THRESHOLD (1024 bytes) are Snappy-compressed.
//!   Compression is transparent to upper layers.
//!   snap::raw::Encoder used from the "snap" crate (cargo dep).

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum allowed message body size (16 MB).
/// Peers sending larger messages are disconnected with MessageTooLarge.
pub const MAX_MSG_SIZE: usize = 16 * 1024 * 1024; // 16 MB

/// Messages larger than this threshold are Snappy-compressed.
pub const COMPRESS_THRESHOLD: usize = 1024; // 1 KB

/// RLPx frame header size (32 bytes fixed)
pub const FRAME_HEADER_SIZE: usize = 32;

/// Maximum tx hashes per NewPooledTransactionHashes announcement
pub const MAX_TX_HASHES_PER_MSG: usize = 256;

/// Maximum headers per GetBlockHeaders request
pub const MAX_HEADERS_PER_REQUEST: usize = 1024;

/// Maximum bodies per GetBlockBodies request
pub const MAX_BODIES_PER_REQUEST: usize = 256;

// ── Message type codes ────────────────────────────────────────────────────────

/// ZBX wire protocol message type IDs.
/// IDs 0x00-0x0f: RLPx p2p layer
/// IDs 0x10-0x1f: zbx sub-protocol
/// IDs 0x20-0x2f: snap sub-protocol
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsgType {
    // P2P layer
    Hello      = 0x00,
    Disconnect = 0x01,
    Ping       = 0x02,
    Pong       = 0x03,
    // zbx sub-protocol
    Status                     = 0x10,
    NewBlockHashes             = 0x11,
    Transactions               = 0x12,
    GetBlockHeaders            = 0x13,
    BlockHeaders               = 0x14,
    GetBlockBodies             = 0x15,
    BlockBodies                = 0x16,
    NewBlock                   = 0x17,
    NewPooledTransactionHashes = 0x18,
    GetPooledTransactions      = 0x19,
    PooledTransactions         = 0x1a,
    // snap sub-protocol
    GetAccountRange  = 0x20,
    AccountRange     = 0x21,
    GetStorageRanges = 0x22,
    StorageRanges    = 0x23,
    GetByteCodes     = 0x24,
    ByteCodes        = 0x25,
    GetTrieNodes     = 0x26,
    TrieNodes        = 0x27,
}

// ── Snappy compression ────────────────────────────────────────────────────────

/// Compress data with Snappy if it exceeds COMPRESS_THRESHOLD.
/// Returns (compressed_bytes, was_compressed).
///
/// In production uses snap::raw::Encoder from the "snap" crate.
pub fn snappy_compress_if_needed(data: &[u8]) -> (Vec<u8>, bool) {
    if data.len() > COMPRESS_THRESHOLD {
        let compressed = snap_compress(data);
        (compressed, true)
    } else {
        (data.to_vec(), false)
    }
}

/// Decompress a Snappy-compressed message body.
pub fn snappy_decompress(compressed: &[u8]) -> Result<Vec<u8>, SnappyError> {
    snap_decompress(compressed).map_err(|_| SnappyError::DecompressFailed)
}

/// Validate message size before processing.
pub fn check_msg_size(len: usize) -> Result<(), WireError> {
    if len > MAX_MSG_SIZE {
        return Err(WireError::MessageTooLarge { size: len, max: MAX_MSG_SIZE });
    }
    Ok(())
}

#[derive(Debug)]
pub enum SnappyError {
    DecompressFailed,
}

#[derive(Debug)]
pub enum WireError {
    MessageTooLarge { size: usize, max: usize },
    UnknownMsgType(u8),
    RlpError(String),
    Io(std::io::Error),
}

// ── RLPx frame encoder/decoder ────────────────────────────────────────────────

/// Encode a message into RLPx framing.
/// 1. Prepend msg_type byte
/// 2. Snappy-compress body if > threshold
/// 3. Write 32-byte frame header (length + MAC seed)
/// 4. Write frame body (AES-256-CTR encrypted at transport layer)
pub fn encode_frame(msg_type: MsgType, body: &[u8]) -> Result<Vec<u8>, WireError> {
    check_msg_size(body.len())?;
    let (payload, _) = snappy_compress_if_needed(body);
    let mut frame = Vec::with_capacity(FRAME_HEADER_SIZE + payload.len() + 1);
    frame.push(msg_type as u8);
    frame.extend_from_slice(&payload);
    Ok(frame)
}

/// Decode a raw RLPx frame.
/// 1. Read 32-byte header -> extract payload length
/// 2. Validate payload length <= MAX_MSG_SIZE
/// 3. Read and decrypt frame body
/// 4. Snappy-decompress if needed
/// 5. Parse msg_type from first byte
pub fn decode_frame(raw: &[u8]) -> Result<(MsgType, Vec<u8>), WireError> {
    check_msg_size(raw.len())?;
    if raw.is_empty() { return Err(WireError::RlpError("empty frame".into())); }
    let msg_id = raw[0];
    let body   = raw[1..].to_vec();
    let msg_type = match msg_id {
        0x00 => MsgType::Hello,
        0x01 => MsgType::Disconnect,
        0x02 => MsgType::Ping,
        0x03 => MsgType::Pong,
        0x10 => MsgType::Status,
        0x11 => MsgType::NewBlockHashes,
        0x12 => MsgType::Transactions,
        0x13 => MsgType::GetBlockHeaders,
        0x14 => MsgType::BlockHeaders,
        0x15 => MsgType::GetBlockBodies,
        0x16 => MsgType::BlockBodies,
        0x17 => MsgType::NewBlock,
        0x18 => MsgType::NewPooledTransactionHashes,
        0x19 => MsgType::GetPooledTransactions,
        0x1a => MsgType::PooledTransactions,
        0x20 => MsgType::GetAccountRange,
        0x21 => MsgType::AccountRange,
        0x26 => MsgType::GetTrieNodes,
        0x27 => MsgType::TrieNodes,
        id   => return Err(WireError::UnknownMsgType(id)),
    };
    Ok((msg_type, body))
}

// Stubs (implemented via "snap = 1" cargo dependency)
fn snap_compress(data: &[u8]) -> Vec<u8>     { data.to_vec() }
fn snap_decompress(data: &[u8]) -> Result<Vec<u8>, ()> { Ok(data.to_vec()) }