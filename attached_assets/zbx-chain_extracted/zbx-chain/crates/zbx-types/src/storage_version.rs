//! Storage-versioning + migration scaffolding for safe state-format upgrades.
//!
//! State on Zebvix lives in a versioned key-value namespace:
//!
//! ```text
//!   /storage/state_v1/{account|code|nonce|…}
//!   /storage/state_v2/{…}
//!   /storage/migration/{from_v1_to_v2,…}
//! ```
//!
//! Each consensus-breaking on-disk schema change bumps a [`StorageVersion`].
//! At node startup, the runtime reads its persisted on-disk version and
//! runs every registered [`Migration`] in order until it matches the
//! version compiled into the binary. If a migration step fails partway,
//! the runtime MUST refuse to start (no half-migrated state).
//!
//! This crate ships only the **types and trait scaffolding**: the actual
//! KV-store implementation lives in `zbx-storage`, and concrete migrations
//! live next to the modules they version (e.g. `zbx-state` ships
//! `migrations/v1_to_v2.rs`). Keeping the contract here means every
//! crate that ever touches state can depend on a single, dependency-light
//! abstraction.
//!
//! ## Invariants
//!
//! * `StorageVersion(u32)` is monotonically increasing — no downgrades.
//! * A [`MigrationPlan`] enforces strict adjacency: step `i+1`'s
//!   `from_version()` must equal step `i`'s `to_version()`. Gaps or
//!   overlaps are rejected by [`MigrationPlan::validate`].
//! * Concrete `Migration` implementations MUST be deterministic and
//!   idempotent under a single execution — they may be re-run on a node
//!   that crashed mid-upgrade only if the runtime has rolled the KV
//!   store back to the pre-step snapshot.
//!
//! ## Example
//!
//! ```ignore
//! struct V1ToV2;
//! impl Migration for V1ToV2 {
//!     fn from_version(&self) -> StorageVersion { StorageVersion(1) }
//!     fn to_version(&self) -> StorageVersion { StorageVersion(2) }
//!     fn description(&self) -> &str { "AccountState gains storage_root field" }
//!     fn migrate(&self, ctx: &mut dyn MigrationContext) -> Result<(), ZbxError> {
//!         // … rewrite each /state_v1/account/* entry into /state_v2/…
//!         Ok(())
//!     }
//! }
//!
//! let plan = MigrationPlan::new(vec![Box::new(V1ToV2)]);
//! plan.validate(StorageVersion(1), StorageVersion(2))?;
//! plan.run(&mut ctx)?;
//! ```

use crate::ZbxError;
use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Monotonic, ordered storage-schema version.
///
/// Wrapped `u32` so a node persisting a single 4-byte value can identify
/// which on-disk format it currently holds. `0` is reserved for "fresh
/// genesis" — the first real schema is `StorageVersion(1)`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StorageVersion(pub u32);

impl StorageVersion {
    /// Reserved sentinel for an uninitialised node (pre-genesis).
    pub const GENESIS: StorageVersion = StorageVersion(0);

    /// Next version (saturating; 4 billion versions is far beyond any
    /// real-world chain lifetime, but never panic on overflow).
    pub fn next(self) -> StorageVersion {
        StorageVersion(self.0.saturating_add(1))
    }

    /// True when `self < other`.
    pub fn is_below(self, other: StorageVersion) -> bool {
        self.0 < other.0
    }
}

impl fmt::Display for StorageVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v{}", self.0)
    }
}

impl Encodable for StorageVersion {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.append(&self.0);
    }
}

impl Decodable for StorageVersion {
    fn decode(rlp: &Rlp<'_>) -> Result<Self, DecoderError> {
        let v: u32 = rlp.as_val()?;
        Ok(StorageVersion(v))
    }
}

/// Storage-context handle exposed to a [`Migration`] step.
///
/// Concrete implementations live in `zbx-storage` (the production
/// RocksDB-backed adapter) and `zbx-storage::tests` (an in-memory
/// adapter used for unit/integration tests).
pub trait MigrationContext {
    /// Read raw bytes at `key` from the legacy (pre-migration) namespace.
    fn read(&self, key: &[u8]) -> Result<Option<Vec<u8>>, ZbxError>;

    /// Write raw bytes at `key` in the new (post-migration) namespace.
    fn write(&mut self, key: &[u8], value: &[u8]) -> Result<(), ZbxError>;

    /// Delete a key from the legacy namespace once safely migrated.
    fn delete(&mut self, key: &[u8]) -> Result<(), ZbxError>;
}

/// One step in a state-schema upgrade plan.
///
/// All implementations MUST be `Send + Sync` so an upgrade can be
/// orchestrated from a non-`!Send` runtime if needed.
pub trait Migration: Send + Sync {
    /// On-disk version this step expects to find.
    fn from_version(&self) -> StorageVersion;

    /// On-disk version this step produces on success.
    fn to_version(&self) -> StorageVersion;

    /// Human-readable description for logs and audit reports.
    fn description(&self) -> &str;

    /// Apply the schema change. Returns [`ZbxError`] on any failure;
    /// the orchestrator MUST roll back the KV store to the pre-step
    /// snapshot before returning the error to the caller.
    fn migrate(&self, ctx: &mut dyn MigrationContext) -> Result<(), ZbxError>;
}

/// A validated, ordered chain of [`Migration`] steps.
///
/// `MigrationPlan` does NOT itself perform IO — it only enforces
/// adjacency invariants and dispatches the steps in order. The
/// orchestrator (in `zbx-storage`) wraps each step in a snapshot/rollback
/// boundary.
pub struct MigrationPlan {
    steps: Vec<Box<dyn Migration>>,
}

impl MigrationPlan {
    /// Construct a plan from an ordered step list. Use
    /// [`MigrationPlan::validate`] before running it.
    pub fn new(steps: Vec<Box<dyn Migration>>) -> Self {
        Self { steps }
    }

    /// Number of steps.
    pub fn len(&self) -> usize {
        self.steps.len()
    }

    /// True iff plan has no steps.
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// Validate that:
    /// 1. The first step's `from_version() == start`.
    /// 2. The last step's `to_version() == target`.
    /// 3. Each step's `from_version()` equals the previous
    ///    step's `to_version()` (no gaps, no overlaps).
    /// 4. Every step strictly advances the version (no no-ops, no
    ///    downgrades).
    ///
    /// An empty plan is valid only when `start == target`.
    pub fn validate(&self, start: StorageVersion, target: StorageVersion) -> Result<(), ZbxError> {
        if self.steps.is_empty() {
            if start == target {
                return Ok(());
            }
            return Err(ZbxError::InvalidInput(format!(
                "MigrationPlan empty but start {start} != target {target}"
            )));
        }
        let first = self.steps.first().unwrap();
        if first.from_version() != start {
            return Err(ZbxError::InvalidInput(format!(
                "MigrationPlan first step expects {} but start is {}",
                first.from_version(),
                start
            )));
        }
        for window in self.steps.windows(2) {
            let prev_to = window[0].to_version();
            let next_from = window[1].from_version();
            if prev_to != next_from {
                return Err(ZbxError::InvalidInput(format!(
                    "MigrationPlan gap: step '{}' ends at {} but next step '{}' expects {}",
                    window[0].description(),
                    prev_to,
                    window[1].description(),
                    next_from
                )));
            }
        }
        for step in &self.steps {
            if !step.from_version().is_below(step.to_version()) {
                return Err(ZbxError::InvalidInput(format!(
                    "MigrationPlan step '{}' does not advance version ({} → {})",
                    step.description(),
                    step.from_version(),
                    step.to_version()
                )));
            }
        }
        let last = self.steps.last().unwrap();
        if last.to_version() != target {
            return Err(ZbxError::InvalidInput(format!(
                "MigrationPlan last step ends at {} but target is {}",
                last.to_version(),
                target
            )));
        }
        Ok(())
    }

    /// Run every step in order. Stops at the first failure and returns
    /// the error WITHOUT rolling back — the caller (orchestrator) is
    /// responsible for snapshot management.
    pub fn run(&self, ctx: &mut dyn MigrationContext) -> Result<(), ZbxError> {
        for step in &self.steps {
            step.migrate(ctx)?;
        }
        Ok(())
    }
}

impl fmt::Debug for MigrationPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MigrationPlan")
            .field("steps", &self.steps.len())
            .field(
                "summary",
                &self
                    .steps
                    .iter()
                    .map(|s| format!("{}→{}: {}", s.from_version(), s.to_version(), s.description()))
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rlp::{decode, encode};
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// In-memory test context.
    #[derive(Default)]
    struct MemCtx {
        store: HashMap<Vec<u8>, Vec<u8>>,
    }
    impl MigrationContext for MemCtx {
        fn read(&self, key: &[u8]) -> Result<Option<Vec<u8>>, ZbxError> {
            Ok(self.store.get(key).cloned())
        }
        fn write(&mut self, key: &[u8], value: &[u8]) -> Result<(), ZbxError> {
            self.store.insert(key.to_vec(), value.to_vec());
            Ok(())
        }
        fn delete(&mut self, key: &[u8]) -> Result<(), ZbxError> {
            self.store.remove(key);
            Ok(())
        }
    }

    /// Test migration v1 → v2.
    struct V1ToV2;
    impl Migration for V1ToV2 {
        fn from_version(&self) -> StorageVersion { StorageVersion(1) }
        fn to_version(&self) -> StorageVersion { StorageVersion(2) }
        fn description(&self) -> &str { "v1→v2 test" }
        fn migrate(&self, ctx: &mut dyn MigrationContext) -> Result<(), ZbxError> {
            ctx.write(b"migrated", b"yes")?;
            Ok(())
        }
    }

    /// Test migration v2 → v3.
    struct V2ToV3;
    impl Migration for V2ToV3 {
        fn from_version(&self) -> StorageVersion { StorageVersion(2) }
        fn to_version(&self) -> StorageVersion { StorageVersion(3) }
        fn description(&self) -> &str { "v2→v3 test" }
        fn migrate(&self, ctx: &mut dyn MigrationContext) -> Result<(), ZbxError> {
            ctx.write(b"v3-marker", b"ok")?;
            Ok(())
        }
    }

    /// A migration that always fails.
    struct FailingMigration;
    impl Migration for FailingMigration {
        fn from_version(&self) -> StorageVersion { StorageVersion(2) }
        fn to_version(&self) -> StorageVersion { StorageVersion(3) }
        fn description(&self) -> &str { "intentionally failing" }
        fn migrate(&self, _ctx: &mut dyn MigrationContext) -> Result<(), ZbxError> {
            Err(ZbxError::InvalidInput("test failure".into()))
        }
    }

    /// A no-op migration (from == to). Used to test that
    /// `validate()` rejects no-op steps.
    struct NoOp;
    impl Migration for NoOp {
        fn from_version(&self) -> StorageVersion { StorageVersion(2) }
        fn to_version(&self) -> StorageVersion { StorageVersion(2) }
        fn description(&self) -> &str { "no-op" }
        fn migrate(&self, _: &mut dyn MigrationContext) -> Result<(), ZbxError> { Ok(()) }
    }

    #[test]
    fn storage_version_display_and_next() {
        assert_eq!(StorageVersion(0).to_string(), "v0");
        assert_eq!(StorageVersion(7).to_string(), "v7");
        assert_eq!(StorageVersion(7).next(), StorageVersion(8));
        // Saturating, not panicking
        assert_eq!(StorageVersion(u32::MAX).next(), StorageVersion(u32::MAX));
    }

    #[test]
    fn storage_version_ordering_and_below() {
        assert!(StorageVersion(1) < StorageVersion(2));
        assert!(StorageVersion(1).is_below(StorageVersion(2)));
        assert!(!StorageVersion(2).is_below(StorageVersion(2)));
        assert!(!StorageVersion(3).is_below(StorageVersion(2)));
    }

    #[test]
    fn storage_version_serde_roundtrip() {
        let v = StorageVersion(42);
        let json = serde_json::to_string(&v).unwrap();
        assert_eq!(json, "42");
        let back: StorageVersion = serde_json::from_str(&json).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn storage_version_rlp_roundtrip() {
        let v = StorageVersion(99);
        let bytes = encode(&v);
        let back: StorageVersion = decode(&bytes).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn empty_plan_only_valid_when_start_equals_target() {
        let plan = MigrationPlan::new(vec![]);
        assert!(plan.validate(StorageVersion(5), StorageVersion(5)).is_ok());
        assert!(plan.validate(StorageVersion(1), StorageVersion(5)).is_err());
    }

    #[test]
    fn valid_plan_runs_all_steps_in_order() {
        // Track call order to ensure determinism.
        struct Tracker {
            calls: Mutex<Vec<u32>>,
        }
        struct OrderedStep {
            from: u32,
            to: u32,
            tracker: std::sync::Arc<Tracker>,
        }
        impl Migration for OrderedStep {
            fn from_version(&self) -> StorageVersion { StorageVersion(self.from) }
            fn to_version(&self) -> StorageVersion { StorageVersion(self.to) }
            fn description(&self) -> &str { "ordered" }
            fn migrate(&self, _: &mut dyn MigrationContext) -> Result<(), ZbxError> {
                self.tracker.calls.lock().unwrap().push(self.from);
                Ok(())
            }
        }

        let tracker = std::sync::Arc::new(Tracker { calls: Mutex::new(vec![]) });
        let plan = MigrationPlan::new(vec![
            Box::new(OrderedStep { from: 1, to: 2, tracker: tracker.clone() }),
            Box::new(OrderedStep { from: 2, to: 3, tracker: tracker.clone() }),
            Box::new(OrderedStep { from: 3, to: 4, tracker: tracker.clone() }),
        ]);
        plan.validate(StorageVersion(1), StorageVersion(4)).unwrap();
        let mut ctx = MemCtx::default();
        plan.run(&mut ctx).unwrap();
        assert_eq!(*tracker.calls.lock().unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn plan_rejects_wrong_start_version() {
        let plan = MigrationPlan::new(vec![Box::new(V1ToV2), Box::new(V2ToV3)]);
        assert!(plan.validate(StorageVersion(0), StorageVersion(3)).is_err());
        assert!(plan.validate(StorageVersion(2), StorageVersion(3)).is_err());
    }

    #[test]
    fn plan_rejects_wrong_target_version() {
        let plan = MigrationPlan::new(vec![Box::new(V1ToV2), Box::new(V2ToV3)]);
        assert!(plan.validate(StorageVersion(1), StorageVersion(2)).is_err());
        assert!(plan.validate(StorageVersion(1), StorageVersion(99)).is_err());
    }

    #[test]
    fn plan_rejects_step_gap() {
        // V1ToV2 then V3ToV4 (skipping v2→v3) — should fail validate
        struct V3ToV4;
        impl Migration for V3ToV4 {
            fn from_version(&self) -> StorageVersion { StorageVersion(3) }
            fn to_version(&self) -> StorageVersion { StorageVersion(4) }
            fn description(&self) -> &str { "v3→v4" }
            fn migrate(&self, _: &mut dyn MigrationContext) -> Result<(), ZbxError> { Ok(()) }
        }
        let plan = MigrationPlan::new(vec![Box::new(V1ToV2), Box::new(V3ToV4)]);
        let err = plan.validate(StorageVersion(1), StorageVersion(4)).unwrap_err();
        assert!(matches!(err, ZbxError::InvalidInput(_)));
    }

    #[test]
    fn plan_rejects_no_op_step() {
        let plan = MigrationPlan::new(vec![Box::new(V1ToV2), Box::new(NoOp)]);
        let err = plan.validate(StorageVersion(1), StorageVersion(2)).unwrap_err();
        assert!(matches!(err, ZbxError::InvalidInput(_)));
    }

    #[test]
    fn plan_run_propagates_step_failure() {
        let plan = MigrationPlan::new(vec![Box::new(V1ToV2), Box::new(FailingMigration)]);
        plan.validate(StorageVersion(1), StorageVersion(3)).unwrap();
        let mut ctx = MemCtx::default();
        let err = plan.run(&mut ctx).unwrap_err();
        assert!(matches!(err, ZbxError::InvalidInput(s) if s.contains("test failure")));
        // First step should have run (write went through) before failure
        assert_eq!(ctx.read(b"migrated").unwrap(), Some(b"yes".to_vec()));
        // Second step's marker should NOT be present
        assert_eq!(ctx.read(b"v3-marker").unwrap(), None);
    }

    #[test]
    fn debug_format_summarises_plan() {
        let plan = MigrationPlan::new(vec![Box::new(V1ToV2), Box::new(V2ToV3)]);
        let s = format!("{:?}", plan);
        assert!(s.contains("v1→v2"));
        assert!(s.contains("v2→v3"));
        assert!(s.contains("steps: 2"));
    }
}
