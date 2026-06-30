use thiserror::Error;
use zbx_ai_precompile::ModelId;

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("model {model_id:?} not found in registry")]
    ModelNotFound { model_id: ModelId },

    #[error("invalid model name: '{0}'")]
    InvalidName(String),

    #[error("invalid DA blob size: {0} bytes")]
    InvalidDaSize(u32),

    #[error("invalid state transition from '{from}' to '{to}'")]
    InvalidTransition { from: String, to: String },

    #[error("insufficient ZBX balance — have {have} wei, need {need} wei")]
    InsufficientBalance { have: u128, need: u128 },

    #[error("proof invalid: {0}")]
    ProofInvalid(String),

    #[error("proposal {0} not found")]
    ProposalNotFound(u64),

    #[error("proposal {0} is not in Active state")]
    ProposalNotActive(u64),

    #[error("proposal {0} has expired")]
    ProposalExpired(u64),

    #[error("address {voter} has already voted on this proposal")]
    AlreadyVoted { voter: String },

    #[error("address {addr} not authorized to {action}")]
    NotAuthorized { addr: String, action: String },

    #[error("model {model_id:?} has too many versions (max 16)")]
    TooManyVersions { model_id: ModelId },

    #[error("registry full: maximum {0} models")]
    RegistryFull(usize),

    #[error("model {model_id:?} already registered at this version")]
    VersionAlreadyExists { model_id: ModelId, version: String },
}
