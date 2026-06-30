//! Ethereum wire protocol messages -- GetReceipts and full protocol suite.
//!
//! ZBX implements the Ethereum wire protocol (eth/68) for block/tx propagation.
//! Messages are RLP-encoded and framed with a message ID byte.
//!
//! Message IDs:
//!   0x00 Status
//!   0x01 NewBlockHashes
//!   0x02 Transactions
//!   0x03 GetBlockHeaders
//!   0x04 BlockHeaders
//!   0x05 GetBlockBodies
//!   0x06 BlockBodies
//!   0x07 NewBlock
//!   0x08 NewPooledTransactionHashes
//!   0x09 GetPooledTransactions
//!   0x0a PooledTransactions
//!   0x0f GetReceipts        <-- fetch tx receipts from a peer
//!   0x10 Receipts           <-- response with receipts
//!
//! GetReceipts / Receipts are used during:
//!   - Light client sync (get receipts without full block body)
//!   - State sync receipt proof verification
//!   - Historical receipt queries from archive nodes

// ── GetReceipts / Receipts ────────────────────────────────────────────────────

/// GetReceipts -- request transaction receipts by block hash.
///
/// A peer responds with all receipts for the transactions in those blocks.
/// Used by light clients and during snap sync (receipt root verification).
#[derive(Debug, Clone)]
pub struct GetReceipts {
    /// Request ID (for matching response)
    pub request_id:   u64,
    /// Block hashes to fetch receipts for (max 256 per request)
    pub block_hashes: Vec<[u8; 32]>,
}

/// Receipts -- response with tx receipts per block.
#[derive(Debug, Clone)]
pub struct ReceiptsResponse {
    pub request_id: u64,
    /// Receipts grouped by block (same order as block_hashes in request).
    /// Each entry is a list of receipts for one block.
    pub receipts:   Vec<Vec<TxReceipt>>,
}

/// Transaction receipt (EIP-658 format).
#[derive(Debug, Clone)]
pub struct TxReceipt {
    /// Transaction hash
    pub tx_hash:           [u8; 32],
    /// Transaction type (0 = legacy, 1 = EIP-2930, 2 = EIP-1559)
    pub tx_type:           u8,
    /// Cumulative gas used in block up to and including this tx
    pub cumulative_gas:    u64,
    /// Bloom filter for logs (256 bytes)
    pub logs_bloom:        [u8; 256],
    /// Logs emitted by this tx
    pub logs:              Vec<ReceiptLog>,
    /// Success (1) or failure (0) -- post EIP-658
    pub status:            u8,
    /// Contract address if this was a CREATE tx, else None
    pub contract_address:  Option<[u8; 20]>,
    /// Gas used by this specific tx
    pub gas_used:          u64,
    /// Effective gas price paid (base_fee + priority_fee)
    pub effective_gas_price: u128,
}

/// A single log (event) entry in a receipt.
#[derive(Debug, Clone)]
pub struct ReceiptLog {
    pub address: [u8; 20],
    pub topics:  Vec<[u8; 32]>,  // max 4 topics
    pub data:    Vec<u8>,         // non-indexed event data
}

/// Receipts request/response handler.
pub struct ReceiptsFetcher {
    pub pending_requests: std::collections::HashMap<u64, Vec<[u8; 32]>>, // request_id -> block_hashes
    pub next_request_id:  u64,
    pub max_blocks_per_req: usize,
}

impl ReceiptsFetcher {
    pub fn new() -> Self {
        Self { pending_requests: std::collections::HashMap::new(), next_request_id: 1, max_blocks_per_req: 256 }
    }

    /// Build a GetReceipts request for a list of block hashes.
    pub fn request_receipts(&mut self, block_hashes: Vec<[u8; 32]>) -> Vec<GetReceipts> {
        let mut requests = Vec::new();
        for chunk in block_hashes.chunks(self.max_blocks_per_req) {
            let req = GetReceipts { request_id: self.next_request_id, block_hashes: chunk.to_vec() };
            self.pending_requests.insert(self.next_request_id, chunk.to_vec());
            self.next_request_id += 1;
            requests.push(req);
        }
        requests
    }

    /// Process a receipts response.
    pub fn on_response(&mut self, resp: ReceiptsResponse) -> Option<Vec<Vec<TxReceipt>>> {
        self.pending_requests.remove(&resp.request_id)?;
        Some(resp.receipts)
    }
}

// ── Protocol version negotiation ──────────────────────────────────────────────

/// Supported Ethereum wire protocol versions.
pub const SUPPORTED_ETH_VERSIONS: &[u32] = &[66, 67, 68];

/// Current protocol version used by ZBX.
pub const CURRENT_ETH_VERSION: u32 = 68;

/// Protocol capability announced during Identify handshake.
pub struct EthProtocolCapability {
    pub name:    &'static str,  // "eth"
    pub version: u32,           // 68
}

pub const ETH_CAPABILITY: EthProtocolCapability = EthProtocolCapability {
    name:    "eth",
    version: CURRENT_ETH_VERSION,
};

// ── Request/response timeout ──────────────────────────────────────────────────

/// Timeout for block/receipt/state sync requests.
pub const REQUEST_TIMEOUT_SECS:  u64 = 15;
pub const RESPONSE_TIMEOUT_SECS: u64 = 15;

/// Pending request tracking (for timeout enforcement).
#[derive(Debug)]
pub struct PendingRequest {
    pub request_id: u64,
    pub peer_id:    String,
    pub sent_at:    u64,
    pub timeout:    u64,
}

impl PendingRequest {
    pub fn is_timed_out(&self, now: u64) -> bool {
        now >= self.sent_at + self.timeout
    }
}