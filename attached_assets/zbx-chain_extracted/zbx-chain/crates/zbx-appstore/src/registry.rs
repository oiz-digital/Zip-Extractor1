//! App registry — primary CRUD interface for the Zebvix App Store.
//!
//! Uses a simple key-value store interface (compatible with RocksDB CF)
//! so it can be swapped for any backing store in tests.

use crate::{
    category::AppCategory,
    error::AppStoreError,
    install::{InstallRecord, InstallTracker},
    metadata::{AppMetadata, AppStatus, ContactInfo},
    rating::{RatingRecord, RatingSummary},
    version::{VersionRecord, VersionStatus},
};
use std::collections::HashMap;

/// In-memory backing store (production: replace with RocksDB CF adapter).
pub struct AppRegistry {
    store: HashMap<String, Vec<u8>>,
}

impl AppRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self { store: HashMap::new() }
    }

    // ── Publish ──────────────────────────────────────────────────────────────

    /// Publish a new application to the store.
    pub fn publish(
        &mut self,
        meta: AppMetadata,
        initial_version: VersionRecord,
    ) -> Result<(), AppStoreError> {
        if !AppMetadata::validate_slug(&meta.slug) {
            return Err(AppStoreError::InvalidSlug(meta.slug.clone()));
        }
        let app_key = format!("apps/{}", meta.slug);
        if self.store.contains_key(&app_key) {
            return Err(AppStoreError::AlreadyExists(meta.slug.clone()));
        }
        VersionRecord::validate_version(&initial_version.version)
            .map_err(|e| AppStoreError::InvalidVersion(initial_version.version.clone(), e))?;

        // Store app metadata.
        let meta_json = serde_json::to_vec(&meta)?;
        self.store.insert(app_key, meta_json);

        // Store initial version.
        let ver_key = format!("apps/{}/versions/{}", meta.slug, initial_version.version);
        let ver_json = serde_json::to_vec(&initial_version)?;
        self.store.insert(ver_key, ver_json);

        // Index under category.
        let cat_key = format!("categories/{}/{}", meta.category.slug(), meta.slug);
        self.store.insert(cat_key, vec![]);

        Ok(())
    }

    // ── Read ─────────────────────────────────────────────────────────────────

    /// Retrieve app metadata by slug.
    pub fn get_app(&self, slug: &str) -> Result<AppMetadata, AppStoreError> {
        let key = format!("apps/{}", slug);
        let bytes = self.store.get(&key).ok_or_else(|| AppStoreError::NotFound(slug.to_string()))?;
        Ok(serde_json::from_slice(bytes)?)
    }

    /// List all apps in a category.
    pub fn list_by_category(&self, category: &AppCategory) -> Vec<String> {
        let prefix = format!("categories/{}/", category.slug());
        self.store
            .keys()
            .filter(|k| k.starts_with(&prefix))
            .map(|k| k[prefix.len()..].to_string())
            .collect()
    }

    /// Get a specific version record.
    pub fn get_version(&self, slug: &str, version: &str) -> Result<VersionRecord, AppStoreError> {
        let key = format!("apps/{}/versions/{}", slug, version);
        let bytes = self.store.get(&key)
            .ok_or_else(|| AppStoreError::VersionNotFound(version.to_string(), slug.to_string()))?;
        Ok(serde_json::from_slice(bytes)?)
    }

    // ── Update ───────────────────────────────────────────────────────────────

    /// Publish a new version for an existing app.
    pub fn publish_version(
        &mut self,
        slug: &str,
        caller: &str,
        version: VersionRecord,
    ) -> Result<(), AppStoreError> {
        let mut meta = self.get_app(slug)?;

        if meta.status == AppStatus::Suspended {
            return Err(AppStoreError::Suspended(slug.to_string()));
        }
        if meta.publisher.to_lowercase() != caller.to_lowercase() {
            return Err(AppStoreError::Unauthorized(caller.to_string(), slug.to_string()));
        }

        VersionRecord::validate_version(&version.version)
            .map_err(|e| AppStoreError::InvalidVersion(version.version.clone(), e))?;

        let ver_key = format!("apps/{}/versions/{}", slug, version.version);
        if self.store.contains_key(&ver_key) {
            return Err(AppStoreError::VersionAlreadyPublished(
                version.version.clone(), slug.to_string(),
            ));
        }

        meta.current_version = version.version.clone();
        meta.updated_at_block = version.published_at_block;

        // Persist updated version + app meta.
        let meta_json = serde_json::to_vec(&meta)?;
        self.store.insert(format!("apps/{}", slug), meta_json);
        let ver_json = serde_json::to_vec(&version)?;
        self.store.insert(ver_key, ver_json);

        Ok(())
    }

    // ── Rating ───────────────────────────────────────────────────────────────

    /// Submit or update a rating for an app.
    pub fn rate_app(
        &mut self,
        slug: &str,
        record: RatingRecord,
    ) -> Result<RatingSummary, AppStoreError> {
        // Verify app exists.
        let _ = self.get_app(slug)?;

        let summary_key = format!("ratings/{}/summary", slug);
        let user_key    = format!("ratings/{}/{}", slug, record.reviewer.to_lowercase());

        let mut summary: RatingSummary = self.store.get(&summary_key)
            .and_then(|b| serde_json::from_slice(b).ok())
            .unwrap_or_default();

        // If user already rated, remove old rating first.
        if let Some(old_bytes) = self.store.get(&user_key) {
            if let Ok(old_rec) = serde_json::from_slice::<RatingRecord>(old_bytes) {
                summary.remove(old_rec.stars);
            }
        }

        summary.add(record.stars);

        let rec_json = serde_json::to_vec(&record)?;
        let sum_json = serde_json::to_vec(&summary)?;
        self.store.insert(user_key, rec_json);
        self.store.insert(summary_key, sum_json);

        Ok(summary)
    }

    /// Get rating summary for an app.
    pub fn get_rating_summary(&self, slug: &str) -> Result<RatingSummary, AppStoreError> {
        let key = format!("ratings/{}/summary", slug);
        Ok(self.store.get(&key)
            .and_then(|b| serde_json::from_slice(b).ok())
            .unwrap_or_default())
    }

    // ── Install tracking ─────────────────────────────────────────────────────

    /// Record an app installation and increment the install counter.
    pub fn record_install(
        &mut self,
        record: InstallRecord,
    ) -> Result<u64, AppStoreError> {
        let _ = self.get_app(&record.slug)?;

        let count_key = InstallTracker::count_key(&record.slug);
        let user_key  = InstallTracker::user_key(&record.slug, &record.user);

        let count = self.store.get(&count_key)
            .map(|b| InstallTracker::decode_count(b))
            .unwrap_or(0) + 1;

        self.store.insert(count_key, InstallTracker::encode_count(count).to_vec());
        let rec_json = serde_json::to_vec(&record)?;
        self.store.insert(user_key, rec_json);

        Ok(count)
    }

    /// Get total install count for an app.
    pub fn install_count(&self, slug: &str) -> u64 {
        let key = InstallTracker::count_key(slug);
        self.store.get(&key)
            .map(|b| InstallTracker::decode_count(b))
            .unwrap_or(0)
    }
}

impl Default for AppRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::AppStatus;

    fn sample_meta(slug: &str, publisher: &str) -> AppMetadata {
        AppMetadata {
            slug: slug.to_string(),
            name: "Test App".to_string(),
            tagline: "A test application".to_string(),
            description: "Longer description".to_string(),
            category: AppCategory::Utilities,
            publisher: publisher.to_string(),
            icon_url: "https://example.com/icon.png".to_string(),
            bundle_url: "ipfs://Qm...".to_string(),
            current_version: "1.0.0".to_string(),
            status: AppStatus::Active,
            tags: vec!["test".to_string()],
            contact: ContactInfo {
                website: None, twitter: None, discord: None,
                github: None, email: None,
            },
            published_at_block: 1000,
            updated_at_block: 1000,
            requires_permissions: vec![],
            min_zbx_required: 0,
        }
    }

    fn sample_version(v: &str, publisher: &str) -> VersionRecord {
        VersionRecord {
            version: v.to_string(),
            bundle_url: "ipfs://Qm...".to_string(),
            bundle_sha256: "a".repeat(64),
            release_notes: "Initial release".to_string(),
            status: VersionStatus::Active,
            published_at_block: 1000,
            publisher: publisher.to_string(),
            min_chain_version: "1.0.0".to_string(),
            bundle_size_bytes: 1_048_576,
        }
    }

    #[test]
    fn publish_and_retrieve() {
        let mut reg = AppRegistry::new();
        let meta = sample_meta("test-app", "0xPublisher");
        let ver  = sample_version("1.0.0", "0xPublisher");
        reg.publish(meta.clone(), ver).unwrap();

        let retrieved = reg.get_app("test-app").unwrap();
        assert_eq!(retrieved.name, "Test App");
    }

    #[test]
    fn duplicate_publish_fails() {
        let mut reg = AppRegistry::new();
        let meta = sample_meta("dup-app", "0xPub");
        let ver  = sample_version("1.0.0", "0xPub");
        reg.publish(meta.clone(), ver.clone()).unwrap();
        assert!(matches!(reg.publish(meta, ver), Err(AppStoreError::AlreadyExists(_))));
    }

    #[test]
    fn category_listing() {
        let mut reg = AppRegistry::new();
        reg.publish(sample_meta("app-a", "0xPub"), sample_version("1.0.0", "0xPub")).unwrap();
        reg.publish(sample_meta("app-b", "0xPub"), sample_version("1.0.0", "0xPub")).unwrap();

        let apps = reg.list_by_category(&AppCategory::Utilities);
        assert_eq!(apps.len(), 2);
        assert!(apps.contains(&"app-a".to_string()));
    }

    #[test]
    fn publish_new_version() {
        let mut reg = AppRegistry::new();
        reg.publish(sample_meta("versioned-app", "0xPub"), sample_version("1.0.0", "0xPub")).unwrap();

        let mut v2 = sample_version("2.0.0", "0xPub");
        v2.published_at_block = 2000;
        reg.publish_version("versioned-app", "0xPub", v2).unwrap();

        let meta = reg.get_app("versioned-app").unwrap();
        assert_eq!(meta.current_version, "2.0.0");
    }

    #[test]
    fn unauthorized_version_publish_fails() {
        let mut reg = AppRegistry::new();
        reg.publish(sample_meta("auth-app", "0xOwner"), sample_version("1.0.0", "0xOwner")).unwrap();
        assert!(matches!(
            reg.publish_version("auth-app", "0xAttacker", sample_version("1.1.0", "0xAttacker")),
            Err(AppStoreError::Unauthorized(_, _))
        ));
    }

    #[test]
    fn rating_and_summary() {
        let mut reg = AppRegistry::new();
        reg.publish(sample_meta("rated-app", "0xPub"), sample_version("1.0.0", "0xPub")).unwrap();

        reg.rate_app("rated-app", RatingRecord::new("0xUser1".into(), 5, None, 1001).unwrap()).unwrap();
        reg.rate_app("rated-app", RatingRecord::new("0xUser2".into(), 3, None, 1002).unwrap()).unwrap();

        let summary = reg.get_rating_summary("rated-app").unwrap();
        assert_eq!(summary.count, 2);
        assert!((summary.average() - 4.0).abs() < 1e-9);
    }

    #[test]
    fn install_tracking() {
        let mut reg = AppRegistry::new();
        reg.publish(sample_meta("installable", "0xPub"), sample_version("1.0.0", "0xPub")).unwrap();

        let count1 = reg.record_install(InstallRecord {
            user: "0xUser1".into(), slug: "installable".into(),
            version: "1.0.0".into(), block_number: 1001,
        }).unwrap();
        assert_eq!(count1, 1);

        let count2 = reg.record_install(InstallRecord {
            user: "0xUser2".into(), slug: "installable".into(),
            version: "1.0.0".into(), block_number: 1002,
        }).unwrap();
        assert_eq!(count2, 2);
        assert_eq!(reg.install_count("installable"), 2);
    }
}
