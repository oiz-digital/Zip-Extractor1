use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProverError {
    // ─── Witness errors ──────────────────────────────────────────────────
    #[error("witness generation failed: {0}")]
    WitnessGeneration(String),

    #[error("execution trace too large: {size} steps, max {max}")]
    TraceTooLarge { size: usize, max: usize },

    #[error("missing state data for address {0}")]
    MissingStateData(String),

    // ─── Circuit errors ───────────────────────────────────────────────────
    #[error("constraint violated at row {row}, column {col}: {msg}")]
    ConstraintViolated { row: usize, col: usize, msg: String },

    #[error("circuit size {size} is not a power of two")]
    CircuitSizeNotPowerOfTwo { size: usize },

    #[error("unsupported circuit type: {0}")]
    UnsupportedCircuit(String),

    // ─── FRI / STARK errors ───────────────────────────────────────────────
    #[error("FRI commitment mismatch at layer {layer}")]
    FriCommitmentMismatch { layer: usize },

    #[error("FRI query out of range: index {index}, domain size {domain}")]
    FriQueryOutOfRange { index: usize, domain: usize },

    #[error("proof verification failed: {0}")]
    VerificationFailed(String),

    #[error("proof version mismatch: expected {expected}, got {got}")]
    ProofVersionMismatch { expected: u8, got: u8 },

    #[error("proof too large: {size} bytes, max {max}")]
    ProofTooLarge { size: usize, max: usize },

    // ─── State proof errors ────────────────────────────────────────────────
    #[error("merkle proof invalid for key {key} at root {root}")]
    MerkleProofInvalid { key: String, root: String },

    #[error("state root mismatch: expected {expected}, got {got}")]
    StateRootMismatch { expected: String, got: String },

    #[error("account not found: {0}")]
    AccountNotFound(String),

    // ─── Fraud proof errors ────────────────────────────────────────────────
    #[error("fraud proof: no mismatch found — execution is correct")]
    NoFraudFound,

    #[error("fraud proof: challenge step {step} out of range [0, {max})")]
    ChallengeOutOfRange { step: usize, max: usize },

    #[error("fraud proof: dispute window expired (submitted at block {submitted}, current {current})")]
    DisputeWindowExpired { submitted: u64, current: u64 },

    // ─── Recursive proof errors ────────────────────────────────────────────
    #[error("recursive: cannot aggregate zero proofs")]
    RecursiveEmptyInput,

    #[error("recursive: proof chain break at index {0}")]
    RecursiveChainBreak(usize),

    // ─── Internal errors ───────────────────────────────────────────────────
    #[error("serialisation error: {0}")]
    Serialisation(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub type ProverResult<T> = Result<T, ProverError>;