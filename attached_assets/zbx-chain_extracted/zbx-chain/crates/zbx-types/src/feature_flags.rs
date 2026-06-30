//! Centralized feature-flag manager.
//!
//! Unlike [`ActivationSchedule`](crate::ActivationSchedule) — which gates
//! consensus-breaking upgrades by chain height — feature flags are
//! **immediate, governance-toggleable boolean switches** for non-consensus
//! behaviour: enabling/disabling new RPC endpoints, opt-in metrics, gradual
//! rollout of mempool tweaks, etc.
//!
//! Both types deliberately share the same name policy and canonical RLP
//! encoding so a single audit script can validate both.
//!
//! ## Invariants (enforced on construct + serde + RLP decode)
//!
//! * `name` matches `[a-z0-9_-]+` (same policy as
//!   [`ModuleVersion`](crate::ModuleVersion) / [`Activation`](crate::Activation)).
//! * `enabled` is a plain `bool` — no monotonicity rule (a flag may flip
//!   either direction at any time).
//! * `FeatureFlags` is alphabetically sorted by `name` for canonical RLP;
//!   **decode rejects unsorted input**.
//!
//! ## Wire formats
//!
//! * JSON / config / genesis: a transparent `BTreeMap<String, bool>`.
//! * RLP / state storage: `[[name_a, enabled_a], …]` strictly sorted, each
//!   row is a 2-item list with the boolean RLP-encoded as 0x00 (false)
//!   or 0x01 (true).
//!
//! ## Usage example
//!
//! ```ignore
//! let mut flags = FeatureFlags::new();
//! flags.set(Flag::new("rpc-trace-call", true)?)?;
//! flags.set(Flag::new("mempool-priority-fee-cap", false)?)?;
//!
//! if flags.is_enabled("rpc-trace-call") {
//!     register_trace_call_handler(rpc);
//! }
//! ```

use crate::ZbxError;
use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// Validate that `name` is a legal flag identifier — same policy as
/// [`crate::ModuleVersion`] and [`crate::Activation`].
fn validate_flag_name(name: &str) -> Result<(), ZbxError> {
    if name.is_empty() {
        return Err(ZbxError::InvalidInput("Flag.name is empty".into()));
    }
    if !name
        .bytes()
        .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'))
    {
        return Err(ZbxError::InvalidInput(format!(
            "Flag.name {name:?} must match [a-z0-9_-]+"
        )));
    }
    Ok(())
}

/// One feature-flag entry: `name → enabled`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct Flag {
    /// Lower-case ASCII flag identifier.
    pub name: String,
    /// Toggle state.
    pub enabled: bool,
}

/// Wire-shape for serde — kept private. Public [`Flag`] always
/// constructs through validation.
#[derive(Deserialize)]
struct FlagRaw {
    name: String,
    enabled: bool,
}

impl<'de> Deserialize<'de> for Flag {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = FlagRaw::deserialize(d)?;
        Flag::new(raw.name, raw.enabled).map_err(serde::de::Error::custom)
    }
}

impl Flag {
    /// Construct a validated `Flag`.
    pub fn new(name: impl Into<String>, enabled: bool) -> Result<Self, ZbxError> {
        let name = name.into();
        validate_flag_name(&name)?;
        Ok(Self { name, enabled })
    }
}

impl fmt::Display for Flag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}={}", self.name, if self.enabled { "on" } else { "off" })
    }
}

impl Encodable for Flag {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(2);
        s.append(&self.name);
        // RLP for bool: 0x00 = false, 0x01 = true. `u8` is the
        // canonical primitive carrier.
        s.append(&(self.enabled as u8));
    }
}

impl Decodable for Flag {
    fn decode(rlp: &Rlp<'_>) -> Result<Self, DecoderError> {
        if rlp.item_count()? != 2 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        let name: String = rlp.val_at(0)?;
        let enabled_byte: u8 = rlp.val_at(1)?;
        let enabled = match enabled_byte {
            0 => false,
            1 => true,
            _ => return Err(DecoderError::Custom("Flag.enabled must be 0 or 1")),
        };
        validate_flag_name(&name).map_err(|_| DecoderError::Custom("Flag.name invalid"))?;
        Ok(Self { name, enabled })
    }
}

/// Canonical, alphabetically-sorted feature-flag registry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct FeatureFlags(BTreeMap<String, bool>);

impl FeatureFlags {
    /// Empty registry.
    pub fn new() -> Self {
        Self(BTreeMap::new())
    }

    /// Set or overwrite a flag. Idempotent.
    pub fn set(&mut self, entry: Flag) -> Result<(), ZbxError> {
        self.0.insert(entry.name, entry.enabled);
        Ok(())
    }

    /// True iff the flag is registered AND enabled. Unknown flags
    /// always return false (deny by default — safe rollout).
    pub fn is_enabled(&self, name: &str) -> bool {
        self.0.get(name).copied().unwrap_or(false)
    }

    /// Lookup raw state (None when unregistered).
    pub fn get(&self, name: &str) -> Option<bool> {
        self.0.get(name).copied()
    }

    /// True when no entries.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Number of registered flags.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Iterate alphabetically.
    pub fn iter(&self) -> impl Iterator<Item = Flag> + '_ {
        self.0.iter().map(|(n, e)| Flag {
            name: n.clone(),
            enabled: *e,
        })
    }
}

impl<'de> Deserialize<'de> for FeatureFlags {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let map = BTreeMap::<String, bool>::deserialize(d)?;
        for k in map.keys() {
            validate_flag_name(k).map_err(serde::de::Error::custom)?;
        }
        Ok(Self(map))
    }
}

impl Encodable for FeatureFlags {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(self.0.len());
        for (name, enabled) in &self.0 {
            s.begin_list(2);
            s.append(name);
            s.append(&(*enabled as u8));
        }
    }
}

impl Decodable for FeatureFlags {
    fn decode(rlp: &Rlp<'_>) -> Result<Self, DecoderError> {
        let mut map = BTreeMap::new();
        let mut prev: Option<String> = None;
        for row in rlp.iter() {
            if row.item_count()? != 2 {
                return Err(DecoderError::RlpIncorrectListLen);
            }
            let name: String = row.val_at(0)?;
            let enabled_byte: u8 = row.val_at(1)?;
            let enabled = match enabled_byte {
                0 => false,
                1 => true,
                _ => return Err(DecoderError::Custom("FeatureFlags entry enabled must be 0 or 1")),
            };
            validate_flag_name(&name)
                .map_err(|_| DecoderError::Custom("FeatureFlags invalid flag name"))?;
            if let Some(p) = &prev {
                if name.as_str() <= p.as_str() {
                    return Err(DecoderError::Custom(
                        "FeatureFlags rows must be strictly alphabetically sorted",
                    ));
                }
            }
            prev = Some(name.clone());
            map.insert(name, enabled);
        }
        Ok(Self(map))
    }
}

impl FromIterator<Flag> for FeatureFlags {
    fn from_iter<I: IntoIterator<Item = Flag>>(iter: I) -> Self {
        let mut out = Self::new();
        for entry in iter {
            out.0.insert(entry.name, entry.enabled);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rlp::{decode, encode};

    #[test]
    fn new_validates_flag_name() {
        assert!(Flag::new("rpc-trace-call", true).is_ok());
        assert!(Flag::new("mempool_v2", false).is_ok());
        assert!(Flag::new("", true).is_err());
        assert!(Flag::new("Has Space", true).is_err());
        assert!(Flag::new("RPC", true).is_err());
        assert!(Flag::new("rpc.trace", true).is_err());
        assert!(Flag::new("emoji_💀", true).is_err());
    }

    #[test]
    fn display_format_is_stable() {
        let on = Flag::new("rpc-trace", true).unwrap();
        let off = Flag::new("mempool-v2", false).unwrap();
        assert_eq!(on.to_string(), "rpc-trace=on");
        assert_eq!(off.to_string(), "mempool-v2=off");
    }

    #[test]
    fn rlp_round_trip_single() {
        let f1 = Flag::new("rpc-trace", true).unwrap();
        let f2 = Flag::new("mempool-v2", false).unwrap();
        let r1: Flag = decode(&encode(&f1)).unwrap();
        let r2: Flag = decode(&encode(&f2)).unwrap();
        assert_eq!(f1, r1);
        assert_eq!(f2, r2);
    }

    #[test]
    fn rlp_decode_rejects_invalid_flag_name() {
        let mut s = RlpStream::new_list(2);
        s.append(&"BAD-NAME".to_string()).append(&1u8);
        let err = decode::<Flag>(&s.out()).unwrap_err();
        assert_eq!(err, DecoderError::Custom("Flag.name invalid"));
    }

    #[test]
    fn rlp_decode_rejects_invalid_bool_byte() {
        let mut s = RlpStream::new_list(2);
        s.append(&"valid-name".to_string()).append(&7u8); // not 0 or 1
        let err = decode::<Flag>(&s.out()).unwrap_err();
        assert_eq!(err, DecoderError::Custom("Flag.enabled must be 0 or 1"));
    }

    #[test]
    fn registry_set_and_query() {
        let mut flags = FeatureFlags::new();
        flags.set(Flag::new("rpc-trace", true).unwrap()).unwrap();
        flags.set(Flag::new("mempool-v2", false).unwrap()).unwrap();
        assert!(flags.is_enabled("rpc-trace"));
        assert!(!flags.is_enabled("mempool-v2"));
        assert!(!flags.is_enabled("unknown")); // deny by default
        assert_eq!(flags.get("rpc-trace"), Some(true));
        assert_eq!(flags.get("missing"), None);
    }

    #[test]
    fn registry_set_can_flip_either_way() {
        let mut flags = FeatureFlags::new();
        flags.set(Flag::new("rpc-trace", true).unwrap()).unwrap();
        flags.set(Flag::new("rpc-trace", false).unwrap()).unwrap();
        assert!(!flags.is_enabled("rpc-trace"));
        flags.set(Flag::new("rpc-trace", true).unwrap()).unwrap();
        assert!(flags.is_enabled("rpc-trace"));
    }

    #[test]
    fn registry_iter_is_alphabetical() {
        let flags: FeatureFlags = vec![
            Flag::new("zoo", true).unwrap(),
            Flag::new("alpha", false).unwrap(),
            Flag::new("mid", true).unwrap(),
        ]
        .into_iter()
        .collect();
        let names: Vec<String> = flags.iter().map(|f| f.name).collect();
        assert_eq!(names, vec!["alpha", "mid", "zoo"]);
    }

    #[test]
    fn registry_rlp_round_trip() {
        let mut flags = FeatureFlags::new();
        flags.set(Flag::new("rpc-trace", true).unwrap()).unwrap();
        flags.set(Flag::new("mempool-v2", false).unwrap()).unwrap();
        flags.set(Flag::new("zvm-debug", true).unwrap()).unwrap();
        let bytes = encode(&flags);
        let decoded: FeatureFlags = decode(&bytes).expect("rlp decode");
        assert_eq!(flags, decoded);
    }

    #[test]
    fn registry_rlp_rejects_unsorted_or_duplicate() {
        let mut s = RlpStream::new_list(2);
        s.begin_list(2).append(&"zoo".to_string()).append(&1u8);
        s.begin_list(2).append(&"alpha".to_string()).append(&0u8);
        let err = decode::<FeatureFlags>(&s.out()).unwrap_err();
        assert_eq!(
            err,
            DecoderError::Custom("FeatureFlags rows must be strictly alphabetically sorted")
        );
        let mut s = RlpStream::new_list(2);
        s.begin_list(2).append(&"dup".to_string()).append(&1u8);
        s.begin_list(2).append(&"dup".to_string()).append(&0u8);
        let err = decode::<FeatureFlags>(&s.out()).unwrap_err();
        assert_eq!(
            err,
            DecoderError::Custom("FeatureFlags rows must be strictly alphabetically sorted")
        );
    }

    #[test]
    fn registry_rlp_rejects_invalid_flag_name() {
        let mut s = RlpStream::new_list(1);
        s.begin_list(2).append(&"BAD".to_string()).append(&1u8);
        let err = decode::<FeatureFlags>(&s.out()).unwrap_err();
        assert_eq!(err, DecoderError::Custom("FeatureFlags invalid flag name"));
    }

    #[test]
    fn registry_rlp_rejects_invalid_bool_byte() {
        let mut s = RlpStream::new_list(1);
        s.begin_list(2).append(&"good-name".to_string()).append(&5u8);
        let err = decode::<FeatureFlags>(&s.out()).unwrap_err();
        assert_eq!(
            err,
            DecoderError::Custom("FeatureFlags entry enabled must be 0 or 1")
        );
    }

    #[test]
    fn json_round_trip() {
        let mut flags = FeatureFlags::new();
        flags.set(Flag::new("rpc-trace", true).unwrap()).unwrap();
        flags.set(Flag::new("mempool-v2", false).unwrap()).unwrap();
        let json = serde_json::to_string(&flags).unwrap();
        assert!(json.contains("\"rpc-trace\":true"));
        assert!(json.contains("\"mempool-v2\":false"));
        let back: FeatureFlags = serde_json::from_str(&json).unwrap();
        assert_eq!(flags, back);
    }

    #[test]
    fn json_deserialize_validates_flag_name() {
        let bad = r#"{"name":"BAD","enabled":true}"#;
        assert!(serde_json::from_str::<Flag>(bad).is_err());
        let bad = r#"{"name":"","enabled":true}"#;
        assert!(serde_json::from_str::<Flag>(bad).is_err());
        let good = r#"{"name":"rpc-trace","enabled":true}"#;
        assert_eq!(
            serde_json::from_str::<Flag>(good).unwrap(),
            Flag::new("rpc-trace", true).unwrap()
        );
    }

    #[test]
    fn json_deserialize_validates_registry_keys() {
        let bad = r#"{"BAD":true,"da":false}"#;
        assert!(serde_json::from_str::<FeatureFlags>(bad).is_err());
        let bad = r#"{"has space":true}"#;
        assert!(serde_json::from_str::<FeatureFlags>(bad).is_err());
        let good = r#"{"alpha":true,"beta":false}"#;
        assert!(serde_json::from_str::<FeatureFlags>(good).is_ok());
    }
}
