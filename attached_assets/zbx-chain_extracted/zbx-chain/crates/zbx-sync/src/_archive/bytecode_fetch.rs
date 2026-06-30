//! Bytecode fetch for state sync (snap/state sync healing phase).
//!
//! During snap sync, after downloading account ranges and storage ranges,
//! the node must also download the bytecode for all smart contracts.
//!
//! Protocol messages (EIP-2481 / snap protocol):
//!   GetBytecodes { request_id, hashes, bytes }
//!   Bytecodes    { request_id, codes }
//!
//! The "hashes" field contains code_hash values from accounts.
//! Peers respond with the raw bytecode for each hash.
//!
//! If a peer doesn't have a requested bytecode, it returns an empty
//! entry for that slot (the syncing node then tries another peer).
//!
//! Trie node healing:
//!   After state download, some trie nodes may be missing (due to
//!   pivot block changes mid-sync). GetTrieNodes fetches these.

// ── GetBytecodes request/response ─────────────────────────────────────────────

/// GetBytecodes -- request bytecode by code_hash.
/// Sent during snap sync healing phase.
#[derive(Debug, Clone)]
pub struct GetBytecodes {
    /// Request ID (for matching response)
    pub request_id: u64,
    /// Code hashes to fetch (keccak256 of contract bytecode)
    pub hashes:     Vec<[u8; 32]>,
    /// Max response size in bytes (to bound memory usage)
    pub bytes:      u64,
}

/// Bytecodes -- response with raw contract bytecodes.
#[derive(Debug, Clone)]
pub struct BytecodesResponse {
    /// Matching request ID
    pub request_id: u64,
    /// Raw bytecodes in the same order as request hashes.
    /// Empty Vec for a hash the peer doesn't have.
    pub codes:      Vec<Vec<u8>>,
}

// ── Trie node healing ─────────────────────────────────────────────────────────

/// GetTrieNodes -- request specific trie nodes by path during healing.
/// Used when some trie nodes are missing after snap sync completes.
#[derive(Debug, Clone)]
pub struct GetTrieNodes {
    pub request_id:  u64,
    /// State root we are healing towards
    pub root:        [u8; 32],
    /// List of (account_hash, storage_paths) to fetch.
    /// Empty storage_paths = fetch account trie node.
    pub paths:       Vec<TrieNodePath>,
    pub bytes:       u64,
}

#[derive(Debug, Clone)]
pub struct TrieNodePath {
    pub account_hash:  [u8; 32],
    /// Storage trie paths within this account (empty = account trie node)
    pub storage_paths: Vec<Vec<u8>>,
}

/// TrieNodes -- response to GetTrieNodes.
#[derive(Debug, Clone)]
pub struct TrieNodesResponse {
    pub request_id: u64,
    /// Raw RLP-encoded trie nodes (in order of request paths).
    pub nodes:      Vec<Vec<u8>>,
}

// ── Bytecode heal manager ─────────────────────────────────────────────────────

/// Manages bytecode fetching during the state sync healing phase.
pub struct BytecodeHealManager {
    /// Pending code_hashes to fetch
    pub pending_hashes:   Vec<[u8; 32]>,
    /// Successfully fetched: hash -> bytecode
    pub fetched:          std::collections::HashMap<[u8; 32], Vec<u8>>,
    /// Hashes currently in-flight (requested from peers)
    pub in_flight:        std::collections::HashSet<[u8; 32]>,
    /// Max batch size per GetBytecodes request
    pub batch_size:       usize,
    /// Max response size per request (bytes)
    pub max_response_bytes: u64,
}

impl BytecodeHealManager {
    pub fn new() -> Self {
        Self {
            pending_hashes:     Vec::new(),
            fetched:            std::collections::HashMap::new(),
            in_flight:          std::collections::HashSet::new(),
            batch_size:         128,
            max_response_bytes: 512 * 1024, // 512 KB per request
        }
    }

    /// Add code hashes that need to be fetched.
    pub fn add_pending(&mut self, hashes: Vec<[u8; 32]>) {
        for h in hashes {
            if !self.fetched.contains_key(&h) && !self.in_flight.contains(&h) {
                self.pending_hashes.push(h);
            }
        }
    }

    /// Build a GetBytecodes request for the next batch.
    pub fn next_request(&mut self, request_id: u64) -> Option<GetBytecodes> {
        if self.pending_hashes.is_empty() { return None; }
        let batch: Vec<[u8; 32]> = self.pending_hashes
            .drain(..self.batch_size.min(self.pending_hashes.len()))
            .collect();
        for h in &batch { self.in_flight.insert(*h); }
        Some(GetBytecodes { request_id, hashes: batch, bytes: self.max_response_bytes })
    }

    /// Process a BytecodesResponse from a peer.
    pub fn on_response(&mut self, req_hashes: &[[u8; 32]], resp: BytecodesResponse) {
        for (hash, code) in req_hashes.iter().zip(resp.codes.iter()) {
            self.in_flight.remove(hash);
            if !code.is_empty() {
                self.fetched.insert(*hash, code.clone());
            } else {
                // Peer doesn't have it -- re-queue for another peer
                self.pending_hashes.push(*hash);
            }
        }
    }

    /// True when all bytecodes have been fetched.
    pub fn is_complete(&self) -> bool {
        self.pending_hashes.is_empty() && self.in_flight.is_empty()
    }

    pub fn progress(&self) -> (usize, usize) {
        (self.fetched.len(), self.fetched.len() + self.pending_hashes.len() + self.in_flight.len())
    }
}