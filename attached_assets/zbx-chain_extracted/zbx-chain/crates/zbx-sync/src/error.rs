use thiserror::Error;

#[derive(Debug, Error)]
pub enum SyncError {
    #[error("peer disconnected during sync")]
    PeerDisconnected,

    #[error("received invalid block at height {0}: {1}")]
    InvalidBlock(u64, String),

    #[error("state chunk {chunk} verification failed: hash mismatch")]
    ChunkHashMismatch { chunk: u64 },

    #[error("pivot block {0} not finalized")]
    PivotNotFinalized(u64),

    #[error("no sync peers available")]
    NoPeers,

    #[error("network error: {0}")]
    Network(String),

    #[error("storage error: {0}")]
    Storage(String),

    #[error("timeout waiting for sync response")]
    Timeout,

    #[error("sync interrupted: {0}")]
    Interrupted(String),

    /// SEC-2026-05-09 Pass-19 (Task #10) — chunk's hash does not
    /// merkle-prove against the manifest's `chunk_root`.
    #[error("chunk {chunk} root does not merkle-prove against manifest.chunk_root")]
    ChunkRootMismatch { chunk: u64 },

    /// SEC-2026-05-09 Pass-19 (Task #10) — manifest's BLS quorum
    /// signature is missing, malformed, or fails pairing verification.
    #[error("snapshot manifest BLS quorum signature invalid: {0}")]
    BadManifestSignature(String),
}