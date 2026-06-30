//! Per-module consensus-version tracking for upgrade coordination.
//!
//! Each on-chain module (state, evm, zvm, da, bundler, payid, …) advertises
//! a monotonically-increasing `version: u32`. The runtime persists the full
//! `ModuleVersions` map in world state at a well-known key so that:
//!
//! 1. **Upgrade migrations** can dispatch on a `(module, from_version) →
//!    handler` matrix (Cosmos `x/upgrade` pattern, adapted for the EVM L1).
//! 2. **Genesis dumps** capture every module's runtime version, making
//!    chain forks deterministic and audit-friendly.
//! 3. **JSON-RPC `zbx_moduleVersions`** can expose the map to clients so
//!    light wallets and indexers refuse to talk to a node running a
//!    consensus version they don't understand.
//!
//! Wire format: `serde` (JSON / genesis) and `rlp` (state storage). Both
//! are length-prefixed and fully canonical — invariants are enforced on
//! **every** decode path (custom `Deserialize`, `Decodable`) so genesis
//! files and state imports cannot smuggle in an invalid name.
//!
//! ## Invariants (enforced on construct + serde decode + rlp decode)
//! * `module` matches `[a-z0-9_-]+` (lowercase ASCII alnum / `_` / `-`,
//!   non-empty). Uppercase, whitespace, punctuation, non-ASCII → rejected.
//! * `version` is strictly increasing across upgrades for a given module
//!   (enforced by `ModuleVersions::set`).
//! * `ModuleVersions` is alphabetically sorted by `module` to give a
//!   canonical RLP encoding; **RLP decode rejects unsorted input** so
//!   that wire-form equality implies semantic equality.

use crate::ZbxError;
use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// Validate that `name` is a legal module identifier.
///
/// Allowed: non-empty `[a-z0-9_-]+`. Rejected: empty, uppercase,
/// whitespace, punctuation other than `_`/`-`, any non-ASCII byte.
fn validate_module_name(name: &str) -> Result<(), ZbxError> {
    if name.is_empty() {
        return Err(ZbxError::InvalidInput("ModuleVersion.module is empty".into()));
    }
    if !name
        .bytes()
        .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'))
    {
        return Err(ZbxError::InvalidInput(format!(
            "ModuleVersion.module {name:?} must match [a-z0-9_-]+"
        )));
    }
    Ok(())
}

/// One row in the on-chain module-version registry.
///
/// Equivalent to Cosmos SDK `x/upgrade/types.ModuleVersion` but encoded
/// for the Zebvix L1 state trie.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct ModuleVersion {
    /// Lower-case ASCII module identifier (e.g. `"evm"`, `"zvm"`, `"da"`).
    pub module: String,
    /// Consensus version. Bumped by every breaking migration.
    pub version: u32,
}

/// Wire-shape for serde deserialisation. Kept private — every public
/// `ModuleVersion` is constructed through validation.
#[derive(Deserialize)]
struct ModuleVersionRaw {
    module: String,
    version: u32,
}

impl<'de> Deserialize<'de> for ModuleVersion {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = ModuleVersionRaw::deserialize(d)?;
        ModuleVersion::new(raw.module, raw.version).map_err(serde::de::Error::custom)
    }
}

impl ModuleVersion {
    /// Construct a validated `ModuleVersion`.
    ///
    /// Returns [`ZbxError::InvalidInput`] when `module` is empty or
    /// contains characters outside `[a-z0-9_-]`.
    pub fn new(module: impl Into<String>, version: u32) -> Result<Self, ZbxError> {
        let module = module.into();
        validate_module_name(&module)?;
        Ok(Self { module, version })
    }

    /// Returns true when `other` is the same module at an equal or higher
    /// consensus version. Used by light clients to validate peer
    /// compatibility before subscribing to gossip.
    pub fn is_compatible_with(&self, other: &Self) -> bool {
        self.module == other.module && other.version >= self.version
    }
}

impl fmt::Display for ModuleVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@v{}", self.module, self.version)
    }
}

impl Encodable for ModuleVersion {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(2);
        s.append(&self.module);
        s.append(&self.version);
    }
}

impl Decodable for ModuleVersion {
    fn decode(rlp: &Rlp<'_>) -> Result<Self, DecoderError> {
        if rlp.item_count()? != 2 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        let module: String = rlp.val_at(0)?;
        let version: u32 = rlp.val_at(1)?;
        validate_module_name(&module)
            .map_err(|_| DecoderError::Custom("ModuleVersion.module invalid"))?;
        Ok(Self { module, version })
    }
}

/// Canonical, alphabetically-sorted registry of every module's consensus
/// version. Serialised as a plain map in JSON and as an RLP list-of-rows
/// in state storage (rows MUST be strictly sorted by `module` —
/// non-canonical encodings are rejected on decode).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ModuleVersions(BTreeMap<String, u32>);

impl ModuleVersions {
    /// Empty registry.
    pub fn new() -> Self {
        Self(BTreeMap::new())
    }

    /// Insert or upgrade a module entry. Rejects downgrades.
    ///
    /// Returns [`ZbxError::InvalidInput`] when the new version is lower
    /// than the existing one (monotonicity). The `entry` itself is
    /// already-validated by virtue of being a constructed `ModuleVersion`.
    pub fn set(&mut self, entry: ModuleVersion) -> Result<(), ZbxError> {
        if let Some(prev) = self.0.get(&entry.module) {
            if entry.version < *prev {
                return Err(ZbxError::InvalidInput(format!(
                    "module {} downgrade rejected: have v{}, got v{}",
                    entry.module, prev, entry.version
                )));
            }
        }
        self.0.insert(entry.module, entry.version);
        Ok(())
    }

    /// Look up a module's current version.
    pub fn get(&self, module: &str) -> Option<u32> {
        self.0.get(module).copied()
    }

    /// True if the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Number of registered modules.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Iterate over every entry in canonical (alphabetical) order.
    pub fn iter(&self) -> impl Iterator<Item = ModuleVersion> + '_ {
        self.0.iter().map(|(m, v)| ModuleVersion {
            module: m.clone(),
            version: *v,
        })
    }
}

impl<'de> Deserialize<'de> for ModuleVersions {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let map = BTreeMap::<String, u32>::deserialize(d)?;
        for k in map.keys() {
            validate_module_name(k).map_err(serde::de::Error::custom)?;
        }
        Ok(Self(map))
    }
}

impl Encodable for ModuleVersions {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(self.0.len());
        for (module, version) in &self.0 {
            s.begin_list(2);
            s.append(module);
            s.append(version);
        }
    }
}

impl Decodable for ModuleVersions {
    fn decode(rlp: &Rlp<'_>) -> Result<Self, DecoderError> {
        let mut map = BTreeMap::new();
        let mut prev: Option<String> = None;
        for row in rlp.iter() {
            if row.item_count()? != 2 {
                return Err(DecoderError::RlpIncorrectListLen);
            }
            let module: String = row.val_at(0)?;
            let version: u32 = row.val_at(1)?;
            validate_module_name(&module)
                .map_err(|_| DecoderError::Custom("ModuleVersions invalid module name"))?;
            if let Some(p) = &prev {
                if module.as_str() <= p.as_str() {
                    return Err(DecoderError::Custom(
                        "ModuleVersions rows must be strictly alphabetically sorted",
                    ));
                }
            }
            prev = Some(module.clone());
            map.insert(module, version);
        }
        Ok(Self(map))
    }
}

impl FromIterator<ModuleVersion> for ModuleVersions {
    fn from_iter<I: IntoIterator<Item = ModuleVersion>>(iter: I) -> Self {
        let mut out = Self::new();
        for entry in iter {
            // FromIterator can't return Result; later entries overwrite earlier
            // ones, matching BTreeMap::from_iter semantics. Validation via
            // `set()` is the recommended path for production code.
            out.0.insert(entry.module, entry.version);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rlp::{decode, encode};

    #[test]
    fn new_validates_module_name() {
        assert!(ModuleVersion::new("evm", 1).is_ok());
        assert!(ModuleVersion::new("zvm-precompile", 7).is_ok());
        assert!(ModuleVersion::new("payid_v2", 9).is_ok());
        assert!(ModuleVersion::new("", 1).is_err());
        assert!(ModuleVersion::new("has space", 1).is_err());
        assert!(ModuleVersion::new("EVM", 1).is_err()); // uppercase
        assert!(ModuleVersion::new("evm.core", 1).is_err()); // punctuation
        assert!(ModuleVersion::new("emoji_💀", 1).is_err()); // non-ASCII
    }

    #[test]
    fn compatibility_check() {
        let want = ModuleVersion::new("evm", 3).unwrap();
        let peer_ok = ModuleVersion::new("evm", 5).unwrap();
        let peer_low = ModuleVersion::new("evm", 2).unwrap();
        let peer_other = ModuleVersion::new("zvm", 5).unwrap();
        assert!(want.is_compatible_with(&peer_ok));
        assert!(!want.is_compatible_with(&peer_low));
        assert!(!want.is_compatible_with(&peer_other));
    }

    #[test]
    fn display_format_is_stable() {
        let v = ModuleVersion::new("payid", 12).unwrap();
        assert_eq!(v.to_string(), "payid@v12");
    }

    #[test]
    fn rlp_round_trip_single() {
        let original = ModuleVersion::new("bundler", 4).unwrap();
        let bytes = encode(&original);
        let decoded: ModuleVersion = decode(&bytes).expect("rlp decode");
        assert_eq!(original, decoded);
    }

    #[test]
    fn rlp_decode_rejects_invalid_module_name() {
        let mut s = RlpStream::new_list(2);
        s.append(&"BAD-UPPER".to_string()).append(&1u32);
        let bytes = s.out();
        let err = decode::<ModuleVersion>(&bytes).unwrap_err();
        assert_eq!(err, DecoderError::Custom("ModuleVersion.module invalid"));
    }

    #[test]
    fn registry_set_rejects_downgrade() {
        let mut reg = ModuleVersions::new();
        reg.set(ModuleVersion::new("evm", 5).unwrap()).unwrap();
        assert!(reg.set(ModuleVersion::new("evm", 4).unwrap()).is_err());
        reg.set(ModuleVersion::new("evm", 5).unwrap()).unwrap(); // equal OK
        reg.set(ModuleVersion::new("evm", 6).unwrap()).unwrap(); // upgrade OK
        assert_eq!(reg.get("evm"), Some(6));
    }

    #[test]
    fn registry_iter_is_alphabetical() {
        let reg: ModuleVersions = vec![
            ModuleVersion::new("zvm", 1).unwrap(),
            ModuleVersion::new("evm", 2).unwrap(),
            ModuleVersion::new("da", 3).unwrap(),
        ]
        .into_iter()
        .collect();
        let names: Vec<String> = reg.iter().map(|m| m.module).collect();
        assert_eq!(names, vec!["da", "evm", "zvm"]);
    }

    #[test]
    fn registry_rlp_round_trip() {
        let mut reg = ModuleVersions::new();
        reg.set(ModuleVersion::new("evm", 2).unwrap()).unwrap();
        reg.set(ModuleVersion::new("zvm", 1).unwrap()).unwrap();
        reg.set(ModuleVersion::new("da", 3).unwrap()).unwrap();
        let bytes = encode(&reg);
        let decoded: ModuleVersions = decode(&bytes).expect("rlp decode");
        assert_eq!(reg, decoded);
    }

    #[test]
    fn registry_rlp_rejects_unsorted_or_duplicate_rows() {
        // Unsorted (zvm before evm)
        let mut s = RlpStream::new_list(2);
        s.begin_list(2).append(&"zvm".to_string()).append(&1u32);
        s.begin_list(2).append(&"evm".to_string()).append(&2u32);
        let err = decode::<ModuleVersions>(&s.out()).unwrap_err();
        assert_eq!(
            err,
            DecoderError::Custom("ModuleVersions rows must be strictly alphabetically sorted")
        );
        // Duplicate (also covered by strict-sort: equal ≤ prev)
        let mut s = RlpStream::new_list(2);
        s.begin_list(2).append(&"evm".to_string()).append(&1u32);
        s.begin_list(2).append(&"evm".to_string()).append(&2u32);
        let err = decode::<ModuleVersions>(&s.out()).unwrap_err();
        assert_eq!(
            err,
            DecoderError::Custom("ModuleVersions rows must be strictly alphabetically sorted")
        );
    }

    #[test]
    fn registry_rlp_rejects_invalid_module_name() {
        let mut s = RlpStream::new_list(1);
        s.begin_list(2).append(&"BAD".to_string()).append(&1u32);
        let err = decode::<ModuleVersions>(&s.out()).unwrap_err();
        assert_eq!(
            err,
            DecoderError::Custom("ModuleVersions invalid module name")
        );
    }

    #[test]
    fn json_round_trip() {
        let mut reg = ModuleVersions::new();
        reg.set(ModuleVersion::new("evm", 2).unwrap()).unwrap();
        reg.set(ModuleVersion::new("payid", 7).unwrap()).unwrap();
        let json = serde_json::to_string(&reg).unwrap();
        assert!(json.contains("\"evm\":2"));
        assert!(json.contains("\"payid\":7"));
        let back: ModuleVersions = serde_json::from_str(&json).unwrap();
        assert_eq!(reg, back);
    }

    #[test]
    fn json_deserialize_validates_module_version_name() {
        // Single ModuleVersion
        let bad = r#"{"module":"BAD","version":1}"#;
        assert!(serde_json::from_str::<ModuleVersion>(bad).is_err());
        let bad = r#"{"module":"","version":1}"#;
        assert!(serde_json::from_str::<ModuleVersion>(bad).is_err());
        let good = r#"{"module":"evm","version":1}"#;
        assert_eq!(
            serde_json::from_str::<ModuleVersion>(good).unwrap(),
            ModuleVersion::new("evm", 1).unwrap()
        );
    }

    #[test]
    fn json_deserialize_validates_registry_keys() {
        // Registry with uppercase key must be rejected
        let bad = r#"{"EVM":1,"da":2}"#;
        assert!(serde_json::from_str::<ModuleVersions>(bad).is_err());
        let bad = r#"{"evm core":1}"#;
        assert!(serde_json::from_str::<ModuleVersions>(bad).is_err());
        let good = r#"{"da":3,"evm":2}"#;
        assert!(serde_json::from_str::<ModuleVersions>(good).is_ok());
    }
}
