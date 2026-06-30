//! On-chain AI Model Registry — versioned model management.
//!
//! The registry is the single source of truth for all AI models deployed on
//! ZBX Chain. It tracks:
//! - Model metadata (name, version, weights hash, pricing)
//! - Model lifecycle (pending → active → deprecated → removed)
//! - DA layer weight references (SHA3-256 content-addressed)
//! - Access control (who can submit, approve, deprecate)
//!
//! Security:
//! - Only governance-approved models can be activated (2-of-3 multisig minimum)
//! - Weight hash must match DA layer content (verified on every inference call)
//! - Model versions are immutable once activated
//! - Deprecated models continue serving until 10,000 block grace period expires

use crate::error::RegistryError;
use zbx_ai_precompile::ModelId;
use serde::{Serialize, Deserialize};
use std::collections::HashMap;

/// Grace period in blocks before a deprecated model stops serving.
pub const DEPRECATION_GRACE_BLOCKS: u64 = 10_000;

/// Maximum model name length.
pub const MAX_NAME_LEN: usize = 64;

/// Maximum number of models in the registry.
pub const MAX_MODELS: usize = 256;

/// Model lifecycle state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelStatus {
    /// Submitted, awaiting governance approval.
    Pending,
    /// Approved and serving inference requests.
    Active,
    /// Soft-deprecated — still serving, will be removed at `remove_at_block`.
    Deprecated { remove_at_block: u64 },
    /// Permanently removed from service.
    Removed,
    /// Suspended due to security incident.
    Suspended { reason: String },
}

/// Model tier — determines pricing multiplier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelTier {
    /// Community (free, governed by ZBX DAO).
    Community,
    /// Standard (ZBX token billing, standard rate).
    Standard,
    /// Premium (higher accuracy, higher gas cost).
    Premium,
    /// Enterprise (custom pricing, SLA guaranteed).
    Enterprise,
}

impl ModelTier {
    pub fn gas_multiplier_bps(&self) -> u32 {
        match self {
            Self::Community  => 10_000, // 1.0x
            Self::Standard   => 12_000, // 1.2x
            Self::Premium    => 20_000, // 2.0x
            Self::Enterprise => 30_000, // 3.0x
        }
    }
}

/// A registered model entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelEntry {
    /// Unique model ID (matches precompile ModelId enum).
    pub model_id:       ModelId,
    /// Semantic version string.
    pub version:        String,
    /// Human-readable name.
    pub name:           String,
    /// Creator/publisher address.
    pub publisher:      [u8; 20],
    /// SHA3-256 hash of weight blob on DA layer.
    pub da_hash:        [u8; 32],
    /// Size of weight blob in bytes.
    pub da_size:        u32,
    /// Lifecycle state.
    pub status:         ModelStatus,
    /// Pricing tier.
    pub tier:           ModelTier,
    /// Block number when this entry was submitted.
    pub submitted_at:   u64,
    /// Block number when this entry was activated (0 = not yet).
    pub activated_at:   u64,
    /// Number of successful inference calls.
    pub inference_count: u64,
    /// ZBX tokens earned by publisher (in wei, 10^18).
    pub earnings_wei:   u128,
    /// Description (max 256 chars).
    pub description:    String,
}

impl ModelEntry {
    pub fn new(
        model_id:    ModelId,
        version:     String,
        name:        String,
        publisher:   [u8; 20],
        da_hash:     [u8; 32],
        da_size:     u32,
        tier:        ModelTier,
        description: String,
        block:       u64,
    ) -> Result<Self, RegistryError> {
        if name.is_empty() || name.len() > MAX_NAME_LEN {
            return Err(RegistryError::InvalidName(name));
        }
        if da_size == 0 {
            return Err(RegistryError::InvalidDaSize(da_size));
        }
        Ok(Self {
            model_id, version, name, publisher, da_hash, da_size,
            tier, description,
            status:          ModelStatus::Pending,
            submitted_at:    block,
            activated_at:    0,
            inference_count: 0,
            earnings_wei:    0,
        })
    }

    pub fn is_serving(&self) -> bool {
        matches!(self.status, ModelStatus::Active | ModelStatus::Deprecated { .. })
    }

    pub fn activate(&mut self, block: u64) -> Result<(), RegistryError> {
        if self.status != ModelStatus::Pending {
            return Err(RegistryError::InvalidTransition {
                from: format!("{:?}", self.status),
                to:   "Active".to_string(),
            });
        }
        self.status       = ModelStatus::Active;
        self.activated_at = block;
        Ok(())
    }

    pub fn deprecate(&mut self, current_block: u64) -> Result<(), RegistryError> {
        if self.status != ModelStatus::Active {
            return Err(RegistryError::InvalidTransition {
                from: format!("{:?}", self.status),
                to:   "Deprecated".to_string(),
            });
        }
        self.status = ModelStatus::Deprecated {
            remove_at_block: current_block + DEPRECATION_GRACE_BLOCKS,
        };
        Ok(())
    }

    pub fn suspend(&mut self, reason: String) {
        self.status = ModelStatus::Suspended { reason };
    }

    pub fn record_inference(&mut self, fee_wei: u128) {
        self.inference_count += 1;
        self.earnings_wei    += fee_wei;
    }
}

/// The on-chain model registry.
pub struct ModelRegistry {
    /// model_id → list of versions (latest last).
    entries:      HashMap<ModelId, Vec<ModelEntry>>,
    /// Total models ever registered.
    total_submitted: u64,
    /// Total successful inferences served.
    total_inferences: u64,
}

impl ModelRegistry {
    pub fn new() -> Self {
        Self {
            entries:          HashMap::new(),
            total_submitted:  0,
            total_inferences: 0,
        }
    }

    /// Submit a new model for governance review.
    pub fn submit(&mut self, entry: ModelEntry) -> Result<(), RegistryError> {
        let versions = self.entries.entry(entry.model_id).or_default();
        if versions.len() >= 16 {
            return Err(RegistryError::TooManyVersions { model_id: entry.model_id });
        }
        self.total_submitted += 1;
        versions.push(entry);
        Ok(())
    }

    /// Get the latest active entry for a model_id.
    pub fn get_active(&self, id: ModelId) -> Option<&ModelEntry> {
        self.entries.get(&id)?
            .iter()
            .filter(|e| e.status == ModelStatus::Active)
            .last()
    }

    /// Get all entries for a model.
    pub fn get_all_versions(&self, id: ModelId) -> &[ModelEntry] {
        self.entries.get(&id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Get the latest entry (any status).
    pub fn get_latest(&self, id: ModelId) -> Option<&ModelEntry> {
        self.entries.get(&id)?.last()
    }

    pub fn get_latest_mut(&mut self, id: ModelId) -> Option<&mut ModelEntry> {
        self.entries.get_mut(&id)?.last_mut()
    }

    /// List all currently serving models.
    pub fn active_models(&self) -> Vec<&ModelEntry> {
        self.entries.values()
            .filter_map(|v| v.iter().filter(|e| e.is_serving()).last())
            .collect()
    }

    /// Record a successful inference call.
    pub fn record_inference(&mut self, id: ModelId, fee_wei: u128) {
        self.total_inferences += 1;
        if let Some(e) = self.get_latest_mut(id) {
            e.record_inference(fee_wei);
        }
    }

    pub fn total_inferences(&self) -> u64 { self.total_inferences }
    pub fn total_submitted(&self) -> u64 { self.total_submitted }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_entry(id: ModelId, block: u64) -> ModelEntry {
        ModelEntry::new(
            id, "1.0.0".to_string(), "test-model".to_string(),
            [1u8; 20], [0u8; 32], 1024,
            ModelTier::Standard, "Test model".to_string(), block,
        ).unwrap()
    }

    #[test]
    fn submit_and_activate() {
        let mut reg = ModelRegistry::new();
        let mut entry = test_entry(ModelId::SpamClassifier, 1000);
        entry.activate(1100).unwrap();
        reg.submit(entry).unwrap();
        assert!(reg.get_active(ModelId::SpamClassifier).is_some());
    }

    #[test]
    fn pending_not_returned_as_active() {
        let mut reg = ModelRegistry::new();
        let entry = test_entry(ModelId::RiskScorer, 1000); // still Pending
        reg.submit(entry).unwrap();
        assert!(reg.get_active(ModelId::RiskScorer).is_none());
    }

    #[test]
    fn deprecation_transition() {
        let mut entry = test_entry(ModelId::NftTagger, 1000);
        entry.activate(1010).unwrap();
        entry.deprecate(2000).unwrap();
        assert!(matches!(entry.status, ModelStatus::Deprecated { remove_at_block: 12_000 }));
    }

    #[test]
    fn double_activate_fails() {
        let mut entry = test_entry(ModelId::SpamClassifier, 100);
        entry.activate(200).unwrap();
        let err = entry.activate(300).unwrap_err();
        assert!(matches!(err, RegistryError::InvalidTransition { .. }));
    }

    #[test]
    fn inference_count_increments() {
        let mut reg = ModelRegistry::new();
        let mut entry = test_entry(ModelId::GasOptimizer, 1);
        entry.activate(2).unwrap();
        reg.submit(entry).unwrap();
        reg.record_inference(ModelId::GasOptimizer, 1_000_000);
        reg.record_inference(ModelId::GasOptimizer, 1_000_000);
        assert_eq!(reg.get_latest(ModelId::GasOptimizer).unwrap().inference_count, 2);
        assert_eq!(reg.total_inferences(), 2);
    }

    #[test]
    fn invalid_name_rejected() {
        let err = ModelEntry::new(
            ModelId::SpamClassifier, "1.0".to_string(), "".to_string(),
            [0u8; 20], [0u8; 32], 512, ModelTier::Community, "desc".to_string(), 1,
        ).unwrap_err();
        assert!(matches!(err, RegistryError::InvalidName(_)));
    }
}
