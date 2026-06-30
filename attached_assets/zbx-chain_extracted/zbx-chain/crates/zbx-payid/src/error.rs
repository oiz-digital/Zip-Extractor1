//! Pay ID error types.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PayIdError {
    #[error("Pay ID '{0}' not found")]
    NotFound(String),

    #[error("Pay ID format invalid: {0}")]
    InvalidFormat(String),

    #[error("Pay ID '{0}' is already taken")]
    AlreadyTaken(String),

    #[error("not authorized to modify '{0}'")]
    Unauthorized(String),

    #[error("unsupported handle '@{0}' — only @zbx is supported on this network")]
    UnsupportedHandle(String),

    #[error("display name is required — please provide your full name (e.g. 'Salman Tyagi')")]
    DisplayNameRequired,

    #[error("display name invalid: {0}")]
    DisplayNameInvalid(String),

    #[error("RPC error: {0}")]
    Rpc(String),

    #[error("contract error: {0}")]
    Contract(String),
}