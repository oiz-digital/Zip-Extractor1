//! Central single-source-of-truth registry composing every upgrade-control
//! surface into a single state-stored object.
//!
//! `VersionRegistry` lives at one well-known key in world state and bundles:
//!
//! 1. [`ModuleVersions`](crate::ModuleVersions) — per-module consensus
//!    version (Cosmos `x/upgrade` shape adapted to the EVM L1).
//! 2. [`ActivationSchedule`](crate::ActivationSchedule) — block-height
//!    feature gating.
//! 3. [`FeatureFlags`](crate::FeatureFlags) — non-consensus boolean
//!    rollout switches.
//! 4. [`StorageVersion`](crate::StorageVersion) — on-disk schema version.
//!
//! Every upgrade governance proposal MUST modify this single object. That
//! makes review easy ("show me the registry diff") and forks deterministic
//! (one `keccak256(rlp(registry))` summarises the entire upgrade surface).
//!
//! ## Invariants (enforced on construct + serde + RLP decode)
//!
//! * Every nested type's invariants are upheld (recursively delegated).
//! * The 4 fields appear in a fixed order in the RLP encoding so the
//!   registry hash is stable across implementations.
//! * Empty registries are valid (genesis state).
//!
//! ## Wire formats
//!
//! * JSON / config / governance proposals: a struct with the four fields.
//! * RLP / state storage: a 4-item list — `[modules, activations, flags,
//!   storage_version]` in that exact order.
//!
//! ## Apply-helper API
//!
//! [`VersionRegistry::apply`] performs an atomic upgrade:
//! every change in the proposal succeeds or none do. On any sub-validation
//! failure, the registry is left untouched (the helper works on a clone
//! and only swaps the result back into `self` on full success).

use crate::{
    activation::{Activation, ActivationSchedule},
    feature_flags::{FeatureFlags, Flag},
    module_version::{ModuleVersion, ModuleVersions},
    storage_version::StorageVersion,
    ZbxError,
};
use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};
use serde::{Deserialize, Serialize};

/// One atomic upgrade proposal.
///
/// `set_modules`, `set_activations`, `set_flags` are `Vec`s rather than
/// the canonical map types so a proposal can carry only the entries it
/// changes (a delta) without having to reproduce the entire registry.
/// `bump_storage` is `Option` because most upgrades don't change the
/// on-disk schema.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegistryUpgrade {
    /// Module versions to insert/upgrade. Each entry is monotonicity-checked
    /// against the existing registry.
    #[serde(default)]
    pub set_modules: Vec<ModuleVersion>,

    /// Activations to insert/reschedule. Governance MAY shift activation
    /// blocks freely (only chain-height comparisons gate runtime usage).
    #[serde(default)]
    pub set_activations: Vec<Activation>,

    /// Feature flags to set or flip.
    #[serde(default)]
    pub set_flags: Vec<Flag>,

    /// New storage version. MUST be strictly greater than the current
    /// storage version, or `None` to leave it unchanged.
    #[serde(default)]
    pub bump_storage: Option<StorageVersion>,
}

impl RegistryUpgrade {
    /// True iff every collection is empty AND no storage bump is requested.
    pub fn is_empty(&self) -> bool {
        self.set_modules.is_empty()
            && self.set_activations.is_empty()
            && self.set_flags.is_empty()
            && self.bump_storage.is_none()
    }
}

/// Composite registry — single source of truth for every upgrade control.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionRegistry {
    pub modules: ModuleVersions,
    pub activations: ActivationSchedule,
    pub flags: FeatureFlags,
    pub storage_version: StorageVersion,
}

impl VersionRegistry {
    /// Empty registry — appropriate for genesis bootstrap.
    pub fn new() -> Self {
        Self::default()
    }

    /// True when no module is registered, no activation is scheduled,
    /// no flag is set, and storage is at GENESIS.
    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
            && self.activations.is_empty()
            && self.flags.is_empty()
            && self.storage_version == StorageVersion::GENESIS
    }

    /// Atomically apply an upgrade proposal.
    ///
    /// Operates on a clone of `self`; only swaps the result back on full
    /// success, so a partial failure leaves the registry untouched.
    ///
    /// Validates:
    /// * Every module entry passes `ModuleVersions::set` (no downgrades).
    /// * The new storage version (if any) is strictly greater than the
    ///   current one.
    /// * All sub-types maintain their own invariants (delegated).
    pub fn apply(&mut self, upgrade: &RegistryUpgrade) -> Result<(), ZbxError> {
        let mut next = self.clone();
        for entry in &upgrade.set_modules {
            next.modules.set(entry.clone())?;
        }
        for entry in &upgrade.set_activations {
            next.activations.set(entry.clone())?;
        }
        for entry in &upgrade.set_flags {
            next.flags.set(entry.clone())?;
        }
        if let Some(new_sv) = upgrade.bump_storage {
            if !next.storage_version.is_below(new_sv) {
                return Err(ZbxError::InvalidInput(format!(
                    "storage_version bump must strictly advance: have {}, got {}",
                    next.storage_version, new_sv
                )));
            }
            next.storage_version = new_sv;
        }
        *self = next;
        Ok(())
    }
}

impl Encodable for VersionRegistry {
    fn rlp_append(&self, s: &mut RlpStream) {
        // Fixed 4-item order. Future additions MUST go at the end so old
        // decoders see RlpIncorrectListLen rather than silently mis-parsing.
        s.begin_list(4);
        s.append(&self.modules);
        s.append(&self.activations);
        s.append(&self.flags);
        s.append(&self.storage_version);
    }
}

impl Decodable for VersionRegistry {
    fn decode(rlp: &Rlp<'_>) -> Result<Self, DecoderError> {
        if rlp.item_count()? != 4 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        Ok(Self {
            modules: rlp.val_at(0)?,
            activations: rlp.val_at(1)?,
            flags: rlp.val_at(2)?,
            storage_version: rlp.val_at(3)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rlp::{decode, encode};

    fn populated() -> VersionRegistry {
        let mut r = VersionRegistry::new();
        let upgrade = RegistryUpgrade {
            set_modules: vec![
                ModuleVersion::new("evm", 2).unwrap(),
                ModuleVersion::new("zvm", 1).unwrap(),
            ],
            set_activations: vec![
                Activation::new("evm-shanghai", 1_000_000).unwrap(),
            ],
            set_flags: vec![Flag::new("rpc-trace", true).unwrap()],
            bump_storage: Some(StorageVersion(1)),
        };
        r.apply(&upgrade).unwrap();
        r
    }

    #[test]
    fn empty_registry_round_trips() {
        let r = VersionRegistry::new();
        assert!(r.is_empty());
        // RLP
        let bytes = encode(&r);
        let back: VersionRegistry = decode(&bytes).unwrap();
        assert_eq!(r, back);
        // JSON
        let json = serde_json::to_string(&r).unwrap();
        let back: VersionRegistry = serde_json::from_str(&json).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn apply_atomically_writes_all_subfields() {
        let r = populated();
        assert_eq!(r.modules.get("evm"), Some(2));
        assert_eq!(r.modules.get("zvm"), Some(1));
        assert!(r.activations.is_active("evm-shanghai", 1_000_000));
        assert!(r.flags.is_enabled("rpc-trace"));
        assert_eq!(r.storage_version, StorageVersion(1));
        assert!(!r.is_empty());
    }

    #[test]
    fn apply_is_atomic_on_failure() {
        // Start from a clean populated registry.
        let mut r = populated();
        let snapshot = r.clone();
        // Try an upgrade that includes a valid module bump AND an invalid
        // storage downgrade. The whole thing must fail and leave r ==
        // snapshot.
        let bad = RegistryUpgrade {
            set_modules: vec![ModuleVersion::new("evm", 5).unwrap()],
            bump_storage: Some(StorageVersion(0)), // downgrade from current 1
            ..Default::default()
        };
        let err = r.apply(&bad).unwrap_err();
        assert!(matches!(err, ZbxError::InvalidInput(_)));
        assert_eq!(r, snapshot, "registry must be unchanged on partial failure");
        // Still v2 (not the would-be v5)
        assert_eq!(r.modules.get("evm"), Some(2));
        assert_eq!(r.storage_version, StorageVersion(1));
    }

    #[test]
    fn apply_rejects_module_downgrade() {
        let mut r = populated(); // evm@v2
        let bad = RegistryUpgrade {
            set_modules: vec![ModuleVersion::new("evm", 1).unwrap()],
            ..Default::default()
        };
        assert!(r.apply(&bad).is_err());
        assert_eq!(r.modules.get("evm"), Some(2)); // unchanged
    }

    #[test]
    fn apply_rejects_storage_downgrade_or_equal() {
        let mut r = populated(); // storage@v1
        let downgrade = RegistryUpgrade {
            bump_storage: Some(StorageVersion(0)),
            ..Default::default()
        };
        assert!(r.apply(&downgrade).is_err());
        let equal = RegistryUpgrade {
            bump_storage: Some(StorageVersion(1)),
            ..Default::default()
        };
        assert!(r.apply(&equal).is_err());
        let advance = RegistryUpgrade {
            bump_storage: Some(StorageVersion(2)),
            ..Default::default()
        };
        assert!(r.apply(&advance).is_ok());
        assert_eq!(r.storage_version, StorageVersion(2));
    }

    #[test]
    fn apply_allows_activation_reschedule() {
        let mut r = populated();
        let reschedule = RegistryUpgrade {
            set_activations: vec![Activation::new("evm-shanghai", 5_000_000).unwrap()],
            ..Default::default()
        };
        r.apply(&reschedule).unwrap();
        assert_eq!(r.activations.get("evm-shanghai"), Some(5_000_000));
    }

    #[test]
    fn apply_can_flip_flags_either_direction() {
        let mut r = populated();
        let flip = RegistryUpgrade {
            set_flags: vec![Flag::new("rpc-trace", false).unwrap()],
            ..Default::default()
        };
        r.apply(&flip).unwrap();
        assert!(!r.flags.is_enabled("rpc-trace"));
    }

    #[test]
    fn apply_empty_upgrade_is_noop_and_succeeds() {
        let mut r = populated();
        let snapshot = r.clone();
        let empty = RegistryUpgrade::default();
        assert!(empty.is_empty());
        r.apply(&empty).unwrap();
        assert_eq!(r, snapshot);
    }

    #[test]
    fn rlp_round_trip_full_registry() {
        let r = populated();
        let bytes = encode(&r);
        let back: VersionRegistry = decode(&bytes).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn rlp_decode_rejects_wrong_field_count() {
        // Encode just 3 fields instead of 4
        let mut s = RlpStream::new_list(3);
        s.append(&ModuleVersions::new());
        s.append(&ActivationSchedule::new());
        s.append(&FeatureFlags::new());
        let err = decode::<VersionRegistry>(&s.out()).unwrap_err();
        assert_eq!(err, DecoderError::RlpIncorrectListLen);
        // Encode 5 fields
        let mut s = RlpStream::new_list(5);
        s.append(&ModuleVersions::new());
        s.append(&ActivationSchedule::new());
        s.append(&FeatureFlags::new());
        s.append(&StorageVersion(1));
        s.append(&"extra".to_string());
        let err = decode::<VersionRegistry>(&s.out()).unwrap_err();
        assert_eq!(err, DecoderError::RlpIncorrectListLen);
    }

    #[test]
    fn json_round_trip_full_registry() {
        let r = populated();
        let json = serde_json::to_string(&r).unwrap();
        let back: VersionRegistry = serde_json::from_str(&json).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn json_deserialize_validates_nested_invariants() {
        // Bad module name should fail the nested ModuleVersions validation
        let bad = r#"{
            "modules": {"BAD": 1},
            "activations": {},
            "flags": {},
            "storage_version": 0
        }"#;
        assert!(serde_json::from_str::<VersionRegistry>(bad).is_err());
    }
}
