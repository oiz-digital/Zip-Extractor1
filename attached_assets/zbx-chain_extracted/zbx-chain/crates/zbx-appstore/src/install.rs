//! Install tracking — counts installs per app and per-user install records.

use serde::{Deserialize, Serialize};

/// Record of a user installing an app at a specific version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallRecord {
    /// Installer wallet address.
    pub user: String,
    /// App slug installed.
    pub slug: String,
    /// Version installed.
    pub version: String,
    /// Block number of installation.
    pub block_number: u64,
}

/// Lightweight tracker wrapping the install counter logic.
pub struct InstallTracker;

impl InstallTracker {
    /// Storage key for the total install count of an app.
    pub fn count_key(slug: &str) -> String {
        format!("installs/{}", slug)
    }

    /// Storage key for a per-user install record.
    pub fn user_key(slug: &str, user: &str) -> String {
        format!("installs/{}/users/{}", slug, user.to_lowercase())
    }

    /// Encode a u64 count as 8 little-endian bytes.
    pub fn encode_count(n: u64) -> [u8; 8] {
        n.to_le_bytes()
    }

    /// Decode 8 little-endian bytes to u64 (returns 0 on short/empty slice).
    pub fn decode_count(bytes: &[u8]) -> u64 {
        if bytes.len() < 8 {
            return 0;
        }
        let mut arr = [0u8; 8];
        arr.copy_from_slice(&bytes[..8]);
        u64::from_le_bytes(arr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_encode_decode_roundtrip() {
        for n in [0u64, 1, 255, 65535, u64::MAX] {
            let enc = InstallTracker::encode_count(n);
            assert_eq!(InstallTracker::decode_count(&enc), n);
        }
    }

    #[test]
    fn decode_short_slice_returns_zero() {
        assert_eq!(InstallTracker::decode_count(&[1, 2, 3]), 0);
        assert_eq!(InstallTracker::decode_count(&[]), 0);
    }

    #[test]
    fn storage_keys_are_deterministic() {
        assert_eq!(InstallTracker::count_key("my-app"), "installs/my-app");
        assert_eq!(
            InstallTracker::user_key("my-app", "0xABCD"),
            "installs/my-app/users/0xabcd"
        );
    }
}
