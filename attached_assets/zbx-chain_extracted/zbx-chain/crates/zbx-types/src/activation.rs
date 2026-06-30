//! Block-height activation schedule for protocol upgrades.
//!
//! Every consensus-breaking change to Zebvix MUST be guarded by an
//! [`Activation`] entry in the on-chain [`ActivationSchedule`]. Code paths
//! consult `schedule.is_active(feature, current_height)` to decide whether
//! to use the new logic or the legacy path. This eliminates ad-hoc "if
//! testnet, use new" branches and gives every node deterministic, identical
//! behaviour at every height.
//!
//! ## Invariants (enforced on construct + serde + RLP decode)
//!
//! * `feature` matches `[a-z0-9_-]+` (same policy as
//!   [`ModuleVersion`](crate::ModuleVersion)).
//! * `block` is a `u64` chain height. There is **no** monotonicity rule on
//!   `set` (governance MAY re-schedule a not-yet-activated feature
//!   forwards or backwards). However, an entry whose `block` is `<=` the
//!   current chain height is considered immutably activated by callers —
//!   the type itself does not enforce that, since it has no clock.
//! * `ActivationSchedule` is alphabetically sorted by `feature` for a
//!   canonical RLP encoding; **RLP decode rejects unsorted input**.
//!
//! ## Wire formats
//!
//! * JSON / genesis: a transparent `BTreeMap<String, u64>`.
//! * RLP / state storage: `[[feature_a, block_a], [feature_b, block_b], …]`
//!   strictly sorted; each row is a 2-item list.
//!
//! ## Usage example
//!
//! ```ignore
//! let mut schedule = ActivationSchedule::new();
//! schedule.set(Activation::new("evm-shanghai", 1_000_000)?)?;
//! schedule.set(Activation::new("zvm-precompiles-v2", 1_500_000)?)?;
//!
//! if schedule.is_active("evm-shanghai", current_height) {
//!     run_shanghai_evm(tx)
//! } else {
//!     run_legacy_evm(tx)
//! }
//! ```

use crate::ZbxError;
use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// Validate that `name` is a legal feature identifier — same policy as
/// [`crate::ModuleVersion`]. Kept private so all decode paths converge here.
fn validate_feature_name(name: &str) -> Result<(), ZbxError> {
    if name.is_empty() {
        return Err(ZbxError::InvalidInput("Activation.feature is empty".into()));
    }
    if !name
        .bytes()
        .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'))
    {
        return Err(ZbxError::InvalidInput(format!(
            "Activation.feature {name:?} must match [a-z0-9_-]+"
        )));
    }
    Ok(())
}

/// One entry in the on-chain activation schedule: at block height `block`,
/// `feature` becomes active. Below that height, callers MUST take the
/// legacy path.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct Activation {
    /// Lower-case ASCII feature identifier (e.g. `"evm-shanghai"`).
    pub feature: String,
    /// Chain height at (and after) which the feature is active.
    pub block: u64,
}

/// Wire-shape for serde — kept private. Public [`Activation`] always
/// constructs through validation.
#[derive(Deserialize)]
struct ActivationRaw {
    feature: String,
    block: u64,
}

impl<'de> Deserialize<'de> for Activation {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = ActivationRaw::deserialize(d)?;
        Activation::new(raw.feature, raw.block).map_err(serde::de::Error::custom)
    }
}

impl Activation {
    /// Construct a validated `Activation`.
    pub fn new(feature: impl Into<String>, block: u64) -> Result<Self, ZbxError> {
        let feature = feature.into();
        validate_feature_name(&feature)?;
        Ok(Self { feature, block })
    }

    /// True when `height >= self.block`.
    pub fn is_active_at(&self, height: u64) -> bool {
        height >= self.block
    }
}

impl fmt::Display for Activation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@block:{}", self.feature, self.block)
    }
}

impl Encodable for Activation {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(2);
        s.append(&self.feature);
        s.append(&self.block);
    }
}

impl Decodable for Activation {
    fn decode(rlp: &Rlp<'_>) -> Result<Self, DecoderError> {
        if rlp.item_count()? != 2 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        let feature: String = rlp.val_at(0)?;
        let block: u64 = rlp.val_at(1)?;
        validate_feature_name(&feature)
            .map_err(|_| DecoderError::Custom("Activation.feature invalid"))?;
        Ok(Self { feature, block })
    }
}

/// Canonical, alphabetically-sorted on-chain activation schedule.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ActivationSchedule(BTreeMap<String, u64>);

impl ActivationSchedule {
    /// Empty schedule.
    pub fn new() -> Self {
        Self(BTreeMap::new())
    }

    /// Insert or re-schedule a feature. Governance MAY move an
    /// activation forwards or backwards (use case: emergency delay).
    /// Callers needing immutability-after-activation must check the
    /// chain height themselves before accepting a re-schedule proposal.
    pub fn set(&mut self, entry: Activation) -> Result<(), ZbxError> {
        self.0.insert(entry.feature, entry.block);
        Ok(())
    }

    /// True iff `feature` is registered AND `height >= its activation`.
    /// Unknown features always return false (deny by default).
    pub fn is_active(&self, feature: &str, height: u64) -> bool {
        self.0.get(feature).is_some_and(|b| height >= *b)
    }

    /// Look up the activation block for a feature.
    pub fn get(&self, feature: &str) -> Option<u64> {
        self.0.get(feature).copied()
    }

    /// True when no entries.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Number of registered features.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Iterate alphabetically.
    pub fn iter(&self) -> impl Iterator<Item = Activation> + '_ {
        self.0.iter().map(|(f, b)| Activation {
            feature: f.clone(),
            block: *b,
        })
    }
}

impl<'de> Deserialize<'de> for ActivationSchedule {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let map = BTreeMap::<String, u64>::deserialize(d)?;
        for k in map.keys() {
            validate_feature_name(k).map_err(serde::de::Error::custom)?;
        }
        Ok(Self(map))
    }
}

impl Encodable for ActivationSchedule {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(self.0.len());
        for (feature, block) in &self.0 {
            s.begin_list(2);
            s.append(feature);
            s.append(block);
        }
    }
}

impl Decodable for ActivationSchedule {
    fn decode(rlp: &Rlp<'_>) -> Result<Self, DecoderError> {
        let mut map = BTreeMap::new();
        let mut prev: Option<String> = None;
        for row in rlp.iter() {
            if row.item_count()? != 2 {
                return Err(DecoderError::RlpIncorrectListLen);
            }
            let feature: String = row.val_at(0)?;
            let block: u64 = row.val_at(1)?;
            validate_feature_name(&feature)
                .map_err(|_| DecoderError::Custom("ActivationSchedule invalid feature name"))?;
            if let Some(p) = &prev {
                if feature.as_str() <= p.as_str() {
                    return Err(DecoderError::Custom(
                        "ActivationSchedule rows must be strictly alphabetically sorted",
                    ));
                }
            }
            prev = Some(feature.clone());
            map.insert(feature, block);
        }
        Ok(Self(map))
    }
}

impl FromIterator<Activation> for ActivationSchedule {
    fn from_iter<I: IntoIterator<Item = Activation>>(iter: I) -> Self {
        let mut out = Self::new();
        for entry in iter {
            out.0.insert(entry.feature, entry.block);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rlp::{decode, encode};

    #[test]
    fn new_validates_feature_name() {
        assert!(Activation::new("evm-shanghai", 100).is_ok());
        assert!(Activation::new("zvm_precompiles_v2", 200).is_ok());
        assert!(Activation::new("", 0).is_err());
        assert!(Activation::new("Has Space", 0).is_err());
        assert!(Activation::new("EVM-SHANGHAI", 0).is_err());
        assert!(Activation::new("evm.shanghai", 0).is_err());
        assert!(Activation::new("emoji_💀", 0).is_err());
    }

    #[test]
    fn is_active_at_boundary() {
        let a = Activation::new("evm-shanghai", 1000).unwrap();
        assert!(!a.is_active_at(999));
        assert!(a.is_active_at(1000));
        assert!(a.is_active_at(1_000_000));
    }

    #[test]
    fn display_format_is_stable() {
        let a = Activation::new("payid-v2", 500_000).unwrap();
        assert_eq!(a.to_string(), "payid-v2@block:500000");
    }

    #[test]
    fn rlp_round_trip_single() {
        let original = Activation::new("bundler-v3", 42).unwrap();
        let bytes = encode(&original);
        let decoded: Activation = decode(&bytes).expect("rlp decode");
        assert_eq!(original, decoded);
    }

    #[test]
    fn rlp_decode_rejects_invalid_feature_name() {
        let mut s = RlpStream::new_list(2);
        s.append(&"BAD-NAME".to_string()).append(&1u64);
        let err = decode::<Activation>(&s.out()).unwrap_err();
        assert_eq!(err, DecoderError::Custom("Activation.feature invalid"));
    }

    #[test]
    fn schedule_set_and_query() {
        let mut sched = ActivationSchedule::new();
        sched.set(Activation::new("evm-shanghai", 1_000_000).unwrap()).unwrap();
        sched.set(Activation::new("zvm-v2", 1_500_000).unwrap()).unwrap();

        // Active checks
        assert!(!sched.is_active("evm-shanghai", 999_999));
        assert!(sched.is_active("evm-shanghai", 1_000_000));
        assert!(sched.is_active("evm-shanghai", 9_000_000));
        assert!(!sched.is_active("zvm-v2", 1_000_000));
        assert!(sched.is_active("zvm-v2", 1_500_000));

        // Unknown feature → deny by default
        assert!(!sched.is_active("unknown-feature", u64::MAX));

        // Lookup
        assert_eq!(sched.get("evm-shanghai"), Some(1_000_000));
        assert_eq!(sched.get("missing"), None);
    }

    #[test]
    fn schedule_set_allows_reschedule() {
        let mut sched = ActivationSchedule::new();
        sched.set(Activation::new("evm-shanghai", 1_000_000).unwrap()).unwrap();
        // Governance moves it forward
        sched.set(Activation::new("evm-shanghai", 2_000_000).unwrap()).unwrap();
        assert_eq!(sched.get("evm-shanghai"), Some(2_000_000));
        // …or backward (emergency hot-fix)
        sched.set(Activation::new("evm-shanghai", 500_000).unwrap()).unwrap();
        assert_eq!(sched.get("evm-shanghai"), Some(500_000));
    }

    #[test]
    fn schedule_iter_is_alphabetical() {
        let sched: ActivationSchedule = vec![
            Activation::new("zvm-v2", 3).unwrap(),
            Activation::new("evm-shanghai", 1).unwrap(),
            Activation::new("da-v2", 2).unwrap(),
        ]
        .into_iter()
        .collect();
        let names: Vec<String> = sched.iter().map(|a| a.feature).collect();
        assert_eq!(names, vec!["da-v2", "evm-shanghai", "zvm-v2"]);
    }

    #[test]
    fn schedule_rlp_round_trip() {
        let mut sched = ActivationSchedule::new();
        sched.set(Activation::new("evm-shanghai", 1_000_000).unwrap()).unwrap();
        sched.set(Activation::new("zvm-v2", 1_500_000).unwrap()).unwrap();
        sched.set(Activation::new("da-v2", 800_000).unwrap()).unwrap();
        let bytes = encode(&sched);
        let decoded: ActivationSchedule = decode(&bytes).expect("rlp decode");
        assert_eq!(sched, decoded);
    }

    #[test]
    fn schedule_rlp_rejects_unsorted_or_duplicate() {
        // Unsorted (zvm before evm)
        let mut s = RlpStream::new_list(2);
        s.begin_list(2).append(&"zvm-v2".to_string()).append(&1u64);
        s.begin_list(2).append(&"evm-shanghai".to_string()).append(&2u64);
        let err = decode::<ActivationSchedule>(&s.out()).unwrap_err();
        assert_eq!(
            err,
            DecoderError::Custom("ActivationSchedule rows must be strictly alphabetically sorted")
        );
        // Duplicate
        let mut s = RlpStream::new_list(2);
        s.begin_list(2).append(&"evm".to_string()).append(&1u64);
        s.begin_list(2).append(&"evm".to_string()).append(&2u64);
        let err = decode::<ActivationSchedule>(&s.out()).unwrap_err();
        assert_eq!(
            err,
            DecoderError::Custom("ActivationSchedule rows must be strictly alphabetically sorted")
        );
    }

    #[test]
    fn schedule_rlp_rejects_invalid_feature_name() {
        let mut s = RlpStream::new_list(1);
        s.begin_list(2).append(&"BAD".to_string()).append(&1u64);
        let err = decode::<ActivationSchedule>(&s.out()).unwrap_err();
        assert_eq!(
            err,
            DecoderError::Custom("ActivationSchedule invalid feature name")
        );
    }

    #[test]
    fn json_round_trip() {
        let mut sched = ActivationSchedule::new();
        sched.set(Activation::new("evm-shanghai", 1_000_000).unwrap()).unwrap();
        sched.set(Activation::new("payid-v2", 7).unwrap()).unwrap();
        let json = serde_json::to_string(&sched).unwrap();
        assert!(json.contains("\"evm-shanghai\":1000000"));
        assert!(json.contains("\"payid-v2\":7"));
        let back: ActivationSchedule = serde_json::from_str(&json).unwrap();
        assert_eq!(sched, back);
    }

    #[test]
    fn json_deserialize_validates_feature_name() {
        let bad = r#"{"feature":"BAD","block":1}"#;
        assert!(serde_json::from_str::<Activation>(bad).is_err());
        let bad = r#"{"feature":"","block":1}"#;
        assert!(serde_json::from_str::<Activation>(bad).is_err());
        let good = r#"{"feature":"evm-shanghai","block":100}"#;
        assert_eq!(
            serde_json::from_str::<Activation>(good).unwrap(),
            Activation::new("evm-shanghai", 100).unwrap()
        );
    }

    #[test]
    fn json_deserialize_validates_schedule_keys() {
        let bad = r#"{"BAD":1,"da":2}"#;
        assert!(serde_json::from_str::<ActivationSchedule>(bad).is_err());
        let bad = r#"{"has space":1}"#;
        assert!(serde_json::from_str::<ActivationSchedule>(bad).is_err());
        let good = r#"{"da-v2":3,"evm-shanghai":1000000}"#;
        assert!(serde_json::from_str::<ActivationSchedule>(good).is_ok());
    }
}
