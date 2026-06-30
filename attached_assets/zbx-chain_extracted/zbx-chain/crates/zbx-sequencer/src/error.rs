use thiserror::Error;

#[derive(Debug, Error)]
pub enum SequencerError {
    #[error("not our turn to propose (slot {slot}, our validator: {validator:?})")]
    NotProposer { slot: u64, validator: [u8; 20] },
    #[error("block exceeds gas limit: used={used}, limit={limit}")]
    GasLimitExceeded { used: u64, limit: u64 },
    #[error("empty block: no transactions selected")]
    EmptyBlock,
    #[error("execution failed during block building: {0}")]
    ExecutionFailed(String),
    #[error("state root mismatch after sealing")]
    StateRootMismatch,
    #[error("slot timer expired before block was sealed")]
    SlotExpired,
    #[error("PBS relay rejected bid: {0}")]
    PbsRejected(String),
    #[error("consensus error: {0}")]
    Consensus(String),
    /// MB-1 fix: secp256k1 signing failure (invalid proposer key).
    #[error("block signing failed: {0}")]
    Signing(String),
}