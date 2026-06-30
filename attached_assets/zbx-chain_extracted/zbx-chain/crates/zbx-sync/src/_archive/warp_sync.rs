//! Warp sync (checkpoint sync) -- fast bootstrap for new nodes.
//!
//! Warp sync allows a new node to skip full block replay and jump directly
//! to a recent finalized checkpoint, downloading only:
//!   1. Checkpoint state (validator set, account balances, contract storage)
//!   2. Block headers from checkpoint to current head
//!   3. Recent blocks (last N) for local verification
//!
//! This reduces sync time from hours/days to minutes.
//!
//! ## Warp sync flow
//!   1. New node connects to peers
//!   2. Requests WarpSyncManifest (list of available warp sync points)
//!   3. Selects a recent finalized checkpoint (e.g. latest)
//!   4. Downloads StateChunks for the checkpoint state
//!   5. Verifies state root matches checkpoint header
//!   6. Downloads block headers from checkpoint to current head
//!   7. Runs LMD-GHOST fork choice to find current head
//!   8. Downloads and verifies recent blocks
//!   9. Node is now synced and can participate in consensus
//!
//! ## State chunks
//!   State is split into chunks of ~4MB each.
//!   Chunks are content-addressed (hash of chunk = key in manifest).
//!   Parallel download from multiple peers.

use std::collections::HashMap;

// ── Warp sync manifest ────────────────────────────────────────────────────────

/// Warp sync manifest -- describes available sync points.
/// Served by full nodes on the /zbx/warp-sync/manifest/1 P2P topic.
#[derive(Debug, Clone)]
pub struct WarpSyncManifest {
    /// Chain ID this manifest is for
    pub chain_id:    u64,
    /// Available warp sync checkpoints (latest first)
    pub checkpoints: Vec<WarpSyncCheckpoint>,
    /// Signature over manifest by a known authority (anti-spam)
    pub signature:   [u8; 65],
}

/// A single warp sync checkpoint (finalized epoch boundary).
#[derive(Debug, Clone)]
pub struct WarpSyncCheckpoint {
    /// Epoch number of this checkpoint
    pub epoch:        u64,
    /// Block number (always epoch_number * EPOCH_LENGTH)
    pub block_number: u64,
    /// Block hash of the checkpoint block
    pub block_hash:   [u8; 32],
    /// State root at this checkpoint
    pub state_root:   [u8; 32],
    /// Aggregated BLS signature proving 2/3+ validators finalized this
    pub agg_sig:      [u8; 96],
    /// Validator bitfield (which validators signed)
    pub signer_bits:  Vec<u8>,
    /// State size in bytes (for progress tracking)
    pub state_bytes:  u64,
    /// Number of state chunks
    pub chunk_count:  u32,
    /// Content hashes of each state chunk
    pub chunk_hashes: Vec<[u8; 32]>,
}

// ── State chunks ──────────────────────────────────────────────────────────────

/// A single state chunk (portion of the state trie at checkpoint).
#[derive(Debug, Clone)]
pub struct StateChunk {
    /// Hash of this chunk (must match manifest)
    pub hash:     [u8; 32],
    /// Chunk index in the manifest
    pub index:    u32,
    /// Raw state data (RLP-encoded trie nodes)
    pub data:     Vec<u8>,
    /// Proof that this chunk belongs to state_root
    pub proof:    Vec<[u8; 32]>,
}

// ── Warp sync state machine ───────────────────────────────────────────────────

/// Current warp sync phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WarpSyncPhase {
    /// Idle (not syncing or already synced)
    Idle,
    /// Downloading warp sync manifests from peers
    FetchingManifest,
    /// Downloading state chunks for selected checkpoint
    DownloadingState {
        checkpoint_epoch: u64,
        chunks_total:     u32,
        chunks_done:      u32,
    },
    /// Verifying downloaded state against state_root
    VerifyingState,
    /// Downloading block headers from checkpoint to head
    FetchingHeaders {
        from_block: u64,
        to_block:   u64,
        done:       u64,
    },
    /// Downloading and verifying recent blocks
    FetchingRecentBlocks {
        count: u64,
        done:  u64,
    },
    /// Warp sync complete -- node is synced
    Complete { synced_to: u64 },
    /// Warp sync failed
    Failed { reason: WarpSyncError },
}

/// Warp sync progress tracker.
pub struct WarpSync {
    pub phase:           WarpSyncPhase,
    pub selected_checkpoint: Option<WarpSyncCheckpoint>,
    pub downloaded_chunks: HashMap<u32, StateChunk>,
    pub peer_manifests:  Vec<(String, WarpSyncManifest)>, // peer_id -> manifest
    pub sync_start_time: u64,
    pub bytes_downloaded: u64,
}

impl WarpSync {
    pub fn new() -> Self {
        Self {
            phase:               WarpSyncPhase::Idle,
            selected_checkpoint: None,
            downloaded_chunks:   HashMap::new(),
            peer_manifests:      Vec::new(),
            sync_start_time:     0,
            bytes_downloaded:    0,
        }
    }

    /// Start warp sync.
    pub fn start(&mut self, now: u64) {
        self.phase = WarpSyncPhase::FetchingManifest;
        self.sync_start_time = now;
    }

    /// Receive a manifest from a peer; select best checkpoint.
    pub fn on_manifest(&mut self, peer_id: String, manifest: WarpSyncManifest) {
        self.peer_manifests.push((peer_id, manifest));
        self.select_best_checkpoint();
    }

    /// Select the checkpoint with the highest epoch from peer manifests.
    fn select_best_checkpoint(&mut self) {
        let best = self.peer_manifests.iter()
            .flat_map(|(_, m)| m.checkpoints.iter())
            .max_by_key(|c| c.epoch)
            .cloned();

        if let Some(cp) = best {
            let total = cp.chunk_count;
            self.selected_checkpoint = Some(cp);
            self.phase = WarpSyncPhase::DownloadingState {
                checkpoint_epoch: self.selected_checkpoint.as_ref().unwrap().epoch,
                chunks_total: total,
                chunks_done: 0,
            };
        }
    }

    /// Record a downloaded state chunk.
    pub fn on_chunk(&mut self, chunk: StateChunk) -> Result<(), WarpSyncError> {
        // Verify chunk hash
        // In real impl: sha256(chunk.data) == chunk.hash
        self.bytes_downloaded += chunk.data.len() as u64;
        let idx = chunk.index;
        self.downloaded_chunks.insert(idx, chunk);

        if let WarpSyncPhase::DownloadingState { chunks_total, ref mut chunks_done, .. } = self.phase {
            *chunks_done += 1;
            if *chunks_done >= chunks_total {
                self.phase = WarpSyncPhase::VerifyingState;
            }
        }
        Ok(())
    }

    /// Warp sync progress percentage (0-100).
    pub fn sync_progress(&self) -> u8 {
        match &self.phase {
            WarpSyncPhase::Idle | WarpSyncPhase::FetchingManifest => 0,
            WarpSyncPhase::DownloadingState { chunks_total, chunks_done, .. } => {
                if *chunks_total == 0 { return 5; }
                5 + (chunks_done * 60 / chunks_total) as u8
            }
            WarpSyncPhase::VerifyingState => 65,
            WarpSyncPhase::FetchingHeaders { from_block, to_block, done } => {
                let total = to_block.saturating_sub(*from_block).max(1);
                65 + (done * 25 / total) as u8
            }
            WarpSyncPhase::FetchingRecentBlocks { count, done } => {
                90 + (done * 10 / count.max(1)) as u8
            }
            WarpSyncPhase::Complete { .. } => 100,
            WarpSyncPhase::Failed { .. } => 0,
        }
    }

    pub fn is_complete(&self) -> bool {
        matches!(self.phase, WarpSyncPhase::Complete { .. })
    }
}

#[derive(Debug, Clone)]
pub enum WarpSyncError {
    NoCheckpointAvailable,
    ChunkHashMismatch { index: u32 },
    StateRootMismatch,
    InvalidSignature,
    PeerDisconnected,
    Timeout,
}