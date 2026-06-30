//! App Store error types.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppStoreError {
    #[error("app '{0}' already exists")]
    AlreadyExists(String),

    #[error("app '{0}' not found")]
    NotFound(String),

    #[error("version '{0}' for app '{1}' not found")]
    VersionNotFound(String, String),

    #[error("version '{0}' already published for app '{1}'")]
    VersionAlreadyPublished(String, String),

    #[error("invalid semver version '{0}': {1}")]
    InvalidVersion(String, String),

    #[error("invalid slug '{0}': must be lowercase alphanumeric with hyphens, max 64 chars")]
    InvalidSlug(String),

    #[error("invalid rating {0}: must be 1–5")]
    InvalidRating(u8),

    #[error("caller {0} is not the publisher of app '{1}'")]
    Unauthorized(String, String),

    #[error("app '{0}' is suspended and cannot be updated")]
    Suspended(String),

    #[error("storage error: {0}")]
    Storage(String),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}
