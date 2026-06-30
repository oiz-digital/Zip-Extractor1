//! App metadata — the primary record stored per application.

use crate::category::AppCategory;
use serde::{Deserialize, Serialize};

/// Publication status of an app in the store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppStatus {
    /// Live and discoverable.
    Active,
    /// Removed by publisher.
    Removed,
    /// Suspended by chain governance.
    Suspended,
}

/// Publisher contact information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContactInfo {
    pub website:  Option<String>,
    pub twitter:  Option<String>,
    pub discord:  Option<String>,
    pub github:   Option<String>,
    pub email:    Option<String>,
}

/// Full metadata record for a registered application.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppMetadata {
    /// Unique kebab-case identifier (e.g. "zebvix-swap").
    pub slug: String,
    /// Human-readable app name.
    pub name: String,
    /// One-line description (max 256 chars).
    pub tagline: String,
    /// Long-form description (max 4096 chars).
    pub description: String,
    /// App category.
    pub category: AppCategory,
    /// Publisher wallet address (hex, EIP-55).
    pub publisher: String,
    /// IPFS CID or HTTPS URL to the app icon (256×256 PNG).
    pub icon_url: String,
    /// IPFS CID or HTTPS URL to the app bundle/manifest.
    pub bundle_url: String,
    /// Current live version string (semver).
    pub current_version: String,
    /// App status in the store.
    pub status: AppStatus,
    /// Tags for full-text search (max 10).
    pub tags: Vec<String>,
    /// Contact information.
    pub contact: ContactInfo,
    /// Block number when first published.
    pub published_at_block: u64,
    /// Block number of the most recent update.
    pub updated_at_block: u64,
    /// Whether the app requires on-chain permissions.
    pub requires_permissions: Vec<String>,
    /// Minimum ZBX required to use the app (0 = free).
    pub min_zbx_required: u128,
}

impl AppMetadata {
    /// Validate the slug format: lowercase alphanumeric + hyphens, 1–64 chars.
    pub fn validate_slug(slug: &str) -> bool {
        !slug.is_empty()
            && slug.len() <= 64
            && slug.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            && !slug.starts_with('-')
            && !slug.ends_with('-')
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_slugs() {
        assert!(AppMetadata::validate_slug("zebvix-swap"));
        assert!(AppMetadata::validate_slug("my-app-123"));
        assert!(AppMetadata::validate_slug("a"));
    }

    #[test]
    fn invalid_slugs() {
        assert!(!AppMetadata::validate_slug(""));
        assert!(!AppMetadata::validate_slug("-starts-with-hyphen"));
        assert!(!AppMetadata::validate_slug("ends-with-hyphen-"));
        assert!(!AppMetadata::validate_slug("UPPERCASE"));
        assert!(!AppMetadata::validate_slug("has spaces"));
        assert!(!AppMetadata::validate_slug(&"a".repeat(65)));
    }
}
