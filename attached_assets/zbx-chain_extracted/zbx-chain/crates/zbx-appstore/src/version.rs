//! App version records — semver-based versioning with release notes.

use serde::{Deserialize, Serialize};

/// Publication status of a specific version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionStatus {
    /// Active and downloadable.
    Active,
    /// Deprecated — still accessible but not recommended.
    Deprecated,
    /// Yanked — blocked due to a critical bug or security issue.
    Yanked,
}

/// Record for a specific published version of an app.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionRecord {
    /// Semver version string (e.g. "1.2.3").
    pub version: String,
    /// IPFS CID or HTTPS URL to the versioned bundle.
    pub bundle_url: String,
    /// SHA-256 checksum of the bundle (hex).
    pub bundle_sha256: String,
    /// Release notes for this version (max 4096 chars).
    pub release_notes: String,
    /// Version status.
    pub status: VersionStatus,
    /// Block number when this version was published.
    pub published_at_block: u64,
    /// Publisher address that signed this release.
    pub publisher: String,
    /// Minimum chain version required (e.g. "1.5.0").
    pub min_chain_version: String,
    /// Size of the bundle in bytes.
    pub bundle_size_bytes: u64,
}

impl VersionRecord {
    /// Validate that `version` is a valid semver string.
    pub fn validate_version(v: &str) -> Result<(), String> {
        semver::Version::parse(v).map(|_| ()).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_semver() {
        assert!(VersionRecord::validate_version("1.0.0").is_ok());
        assert!(VersionRecord::validate_version("0.1.0-alpha.1").is_ok());
        assert!(VersionRecord::validate_version("2.3.4+build.5").is_ok());
    }

    #[test]
    fn invalid_semver() {
        assert!(VersionRecord::validate_version("1.0").is_err());
        assert!(VersionRecord::validate_version("not-semver").is_err());
        assert!(VersionRecord::validate_version("").is_err());
    }
}
