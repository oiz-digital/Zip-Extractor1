//! Oracle hardening types — multi-source signed-data envelopes,
//! aggregation rules (median / trimmed-mean), staleness/deviation policy,
//! and the canonical `OracleReject` reason.
//!
//! Type-and-codec layer. The actual oracle aggregator lives off-chain
//! (zbx-oracle service); on-chain consumers verify signed feeds against
//! `OraclePolicy`.
//!
//! Discipline (matches sibling modules):
//! - `BTreeMap`/`BTreeSet` for canonical RLP. `validate()` runs in BOTH
//!   constructor AND `Decodable::decode`.
//! - `s.append(&inner)` inside `begin_list(N)` — never the naked
//!   `inner.rlp_append(s)`, which silently skips the parent counter.
//! - Newtype `Encodable` impls use `self.inner.rlp_append(s)` for direct
//!   delegation (LESSON #11).
//! - Prices are `u128` encoded as 16-byte BE (LESSON: rlp crate has no
//!   built-in u128 codec).

use std::collections::{BTreeMap, BTreeSet};

use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};
use serde::{Deserialize, Serialize};

use crate::address::Address;

// ---------------------------------------------------------------------------
// FeedId — opaque 32-char ASCII identifier for an oracle feed
// (e.g. "ZBX/USD", "ETH/USD").
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct FeedId(pub String);

impl FeedId {
    pub const MAX_LEN: usize = 32;

    pub fn new(s: impl Into<String>) -> Result<Self, DecoderError> {
        let s = s.into();
        let f = Self(s);
        f.validate()?;
        Ok(f)
    }

    pub fn validate(&self) -> Result<(), DecoderError> {
        if self.0.is_empty() {
            return Err(DecoderError::Custom("FeedId must be non-empty"));
        }
        if self.0.len() > Self::MAX_LEN {
            return Err(DecoderError::Custom("FeedId exceeds 32 chars"));
        }
        if !self.0.chars().all(|c| c.is_ascii_graphic()) {
            return Err(DecoderError::Custom(
                "FeedId must be printable ASCII",
            ));
        }
        Ok(())
    }
}

impl Encodable for FeedId {
    fn rlp_append(&self, s: &mut RlpStream) {
        // LESSON #11: direct delegation, not s.append(&String).
        self.0.rlp_append(s);
    }
}

impl Decodable for FeedId {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        let raw: String = rlp.as_val()?;
        let f = Self(raw);
        f.validate()?;
        Ok(f)
    }
}

// ---------------------------------------------------------------------------
// AggregationKind — how the on-chain consumer combines N submissions.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum AggregationKind {
    /// Simple median (rejects ties at floor — even-N picks lower).
    Median,
    /// Trimmed mean: drop top-k and bottom-k, average the rest.
    TrimmedMean { trim_each_side: u8 },
    /// Weighted average by submitter stake (off-chain weights).
    StakeWeightedMean,
}

impl AggregationKind {
    pub fn tag(&self) -> u8 {
        match self {
            Self::Median => 0,
            Self::TrimmedMean { .. } => 1,
            Self::StakeWeightedMean => 2,
        }
    }
}

impl Encodable for AggregationKind {
    fn rlp_append(&self, s: &mut RlpStream) {
        match self {
            Self::Median | Self::StakeWeightedMean => {
                s.begin_list(1);
                s.append(&self.tag());
            }
            Self::TrimmedMean { trim_each_side } => {
                s.begin_list(2);
                s.append(&self.tag());
                s.append(trim_each_side);
            }
        }
    }
}

impl Decodable for AggregationKind {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        let n = rlp.item_count()?;
        if n == 0 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        let tag: u8 = rlp.val_at(0)?;
        match tag {
            0 if n == 1 => Ok(Self::Median),
            1 if n == 2 => Ok(Self::TrimmedMean {
                trim_each_side: rlp.val_at(1)?,
            }),
            2 if n == 1 => Ok(Self::StakeWeightedMean),
            _ => Err(DecoderError::Custom(
                "AggregationKind: unknown tag or arity mismatch",
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// OracleSubmission — one signed price report from a single oracle node.
//
// The signature itself lives off-chain and is verified by the aggregator;
// on-chain we only carry the canonical wire-form. Price is u128 (≥1e18 supports
// 18-decimal fixed-point well beyond any plausible asset).
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OracleSubmission {
    pub feed: FeedId,
    pub submitter: Address,
    /// Block height at which the submitter signed this report.
    pub source_height: u64,
    /// Median timestamp from the off-chain source(s), milliseconds.
    pub source_timestamp_ms: u64,
    /// Reported value, fixed-point (decimals decided by feed).
    pub value: u128,
}

impl OracleSubmission {
    pub fn new(
        feed: FeedId,
        submitter: Address,
        source_height: u64,
        source_timestamp_ms: u64,
        value: u128,
    ) -> Result<Self, DecoderError> {
        let v = Self {
            feed,
            submitter,
            source_height,
            source_timestamp_ms,
            value,
        };
        v.validate()?;
        Ok(v)
    }

    pub fn validate(&self) -> Result<(), DecoderError> {
        self.feed.validate()?;
        if self.source_height == 0 {
            return Err(DecoderError::Custom("source_height must be > 0"));
        }
        if self.source_timestamp_ms == 0 {
            return Err(DecoderError::Custom(
                "source_timestamp_ms must be > 0",
            ));
        }
        Ok(())
    }
}

fn append_address(s: &mut RlpStream, a: &Address) {
    s.append(&a.0.as_ref());
}
fn decode_address(rlp: &Rlp) -> Result<Address, DecoderError> {
    let bytes: Vec<u8> = rlp.as_val()?;
    if bytes.len() != 20 {
        return Err(DecoderError::Custom("address must be 20 bytes"));
    }
    let mut out = [0u8; 20];
    out.copy_from_slice(&bytes);
    Ok(Address(out))
}

fn append_u128(s: &mut RlpStream, v: u128) {
    let bytes = v.to_be_bytes();
    s.append(&bytes.as_slice());
}
fn decode_u128(rlp: &Rlp) -> Result<u128, DecoderError> {
    let bytes: Vec<u8> = rlp.as_val()?;
    if bytes.len() != 16 {
        return Err(DecoderError::Custom("u128 must be 16 bytes BE"));
    }
    let mut buf = [0u8; 16];
    buf.copy_from_slice(&bytes);
    Ok(u128::from_be_bytes(buf))
}

impl Encodable for OracleSubmission {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(5);
        s.append(&self.feed);
        append_address(s, &self.submitter);
        s.append(&self.source_height);
        s.append(&self.source_timestamp_ms);
        append_u128(s, self.value);
    }
}

impl Decodable for OracleSubmission {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if rlp.item_count()? != 5 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        let v = Self {
            feed: rlp.val_at(0)?,
            submitter: decode_address(&rlp.at(1)?)?,
            source_height: rlp.val_at(2)?,
            source_timestamp_ms: rlp.val_at(3)?,
            value: decode_u128(&rlp.at(4)?)?,
        };
        v.validate()?;
        Ok(v)
    }
}

// ---------------------------------------------------------------------------
// OracleFeedSpec — per-feed parameters.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OracleFeedSpec {
    pub id: FeedId,
    /// Aggregation rule applied to N submissions.
    pub aggregation: AggregationKind,
    /// Decimal places (e.g. 8 for USD prices, 18 for ETH).
    pub decimals: u8,
    /// Minimum number of distinct oracle submitters required for a publish.
    pub min_submissions: u8,
    /// Maximum allowed staleness, in source-clock milliseconds, before the
    /// feed is rejected as stale.
    pub max_staleness_ms: u64,
    /// Maximum allowed deviation between consecutive publishes, in basis
    /// points (1bp = 0.01%). A 5_000 value caps moves at 50%.
    pub max_deviation_bps: u32,
    /// Authorized submitter set (addresses). MUST be sorted strictly ascending.
    pub authorized_submitters: BTreeSet<Address>,
}

impl OracleFeedSpec {
    pub fn validate(&self) -> Result<(), DecoderError> {
        self.id.validate()?;
        if self.decimals > 30 {
            return Err(DecoderError::Custom("decimals must be <= 30"));
        }
        if self.min_submissions == 0 {
            return Err(DecoderError::Custom("min_submissions must be > 0"));
        }
        if (self.authorized_submitters.len() as u32)
            < self.min_submissions as u32
        {
            return Err(DecoderError::Custom(
                "authorized_submitters must contain >= min_submissions entries",
            ));
        }
        if self.max_staleness_ms == 0 {
            return Err(DecoderError::Custom("max_staleness_ms must be > 0"));
        }
        if self.max_deviation_bps == 0 {
            return Err(DecoderError::Custom(
                "max_deviation_bps must be > 0",
            ));
        }
        if self.max_deviation_bps > 100_000 {
            return Err(DecoderError::Custom(
                "max_deviation_bps must be <= 100000 (1000%)",
            ));
        }
        if let AggregationKind::TrimmedMean { trim_each_side } =
            self.aggregation
        {
            // Need at least 2*trim+1 submissions to leave anyone in.
            let need = (trim_each_side as u32).saturating_mul(2).saturating_add(1);
            if (self.min_submissions as u32) < need {
                return Err(DecoderError::Custom(
                    "TrimmedMean: min_submissions must be >= 2*trim+1",
                ));
            }
        }
        Ok(())
    }
}

impl Encodable for OracleFeedSpec {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(7);
        s.append(&self.id);
        s.append(&self.aggregation);
        s.append(&self.decimals);
        s.append(&self.min_submissions);
        s.append(&self.max_staleness_ms);
        s.append(&self.max_deviation_bps);
        s.begin_list(self.authorized_submitters.len());
        for a in &self.authorized_submitters {
            append_address(s, a);
        }
    }
}

impl Decodable for OracleFeedSpec {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if rlp.item_count()? != 7 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        let id: FeedId = rlp.val_at(0)?;
        let aggregation: AggregationKind = rlp.val_at(1)?;
        let decimals: u8 = rlp.val_at(2)?;
        let min_submissions: u8 = rlp.val_at(3)?;
        let max_staleness_ms: u64 = rlp.val_at(4)?;
        let max_deviation_bps: u32 = rlp.val_at(5)?;

        let mut authorized_submitters = BTreeSet::new();
        let mut prev: Option<Address> = None;
        for item in rlp.at(6)?.iter() {
            let a = decode_address(&item)?;
            if let Some(ref p) = prev {
                if &a <= p {
                    return Err(DecoderError::Custom(
                        "authorized_submitters must be strictly ascending",
                    ));
                }
            }
            prev = Some(a);
            authorized_submitters.insert(a);
        }

        let s_ = Self {
            id,
            aggregation,
            decimals,
            min_submissions,
            max_staleness_ms,
            max_deviation_bps,
            authorized_submitters,
        };
        s_.validate()?;
        Ok(s_)
    }
}

// ---------------------------------------------------------------------------
// OraclePolicy — the chain-wide registry of feed specs.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct OraclePolicy {
    feeds: BTreeMap<FeedId, OracleFeedSpec>,
}

impl OraclePolicy {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert(&mut self, spec: OracleFeedSpec) -> Result<(), DecoderError> {
        spec.validate()?;
        self.feeds.insert(spec.id.clone(), spec);
        Ok(())
    }

    pub fn get(&self, id: &FeedId) -> Option<&OracleFeedSpec> {
        self.feeds.get(id)
    }

    pub fn len(&self) -> usize {
        self.feeds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.feeds.is_empty()
    }

    pub fn feeds(&self) -> &BTreeMap<FeedId, OracleFeedSpec> {
        &self.feeds
    }
}

impl Encodable for OraclePolicy {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(self.feeds.len());
        for (id, spec) in &self.feeds {
            s.begin_list(2);
            s.append(id);
            s.append(spec);
        }
    }
}

impl Decodable for OraclePolicy {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        let mut feeds = BTreeMap::new();
        let mut prev: Option<FeedId> = None;
        for item in rlp.iter() {
            if item.item_count()? != 2 {
                return Err(DecoderError::RlpIncorrectListLen);
            }
            let id: FeedId = item.val_at(0)?;
            let spec: OracleFeedSpec = item.val_at(1)?;
            if spec.id != id {
                return Err(DecoderError::Custom(
                    "OraclePolicy key/value FeedId mismatch",
                ));
            }
            if let Some(ref p) = prev {
                if &id <= p {
                    return Err(DecoderError::Custom(
                        "OraclePolicy must be strictly ascending by FeedId",
                    ));
                }
            }
            prev = Some(id.clone());
            feeds.insert(id, spec);
        }
        Ok(Self { feeds })
    }
}

// ---------------------------------------------------------------------------
// OracleReject — canonical rejection reasons for an aggregated publish.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum OracleReject {
    /// Fewer than `min_submissions` distinct submitters.
    NotEnoughSubmissions { got: u8, need: u8 },
    /// One or more submitters not in `authorized_submitters`.
    UnauthorizedSubmitter,
    /// Newest submission older than `max_staleness_ms` from current time.
    Stale { age_ms: u64, max_ms: u64 },
    /// Deviation from previous publish exceeds `max_deviation_bps`.
    DeviationExceeded { delta_bps: u32, max_bps: u32 },
    /// Trim parameter invalid for actual submission count.
    TrimmedMeanInsufficient { got: u8, need: u8 },
    /// Feed is not in the policy registry.
    UnknownFeed,
}

impl OracleReject {
    pub fn tag(&self) -> u8 {
        match self {
            Self::NotEnoughSubmissions { .. } => 0,
            Self::UnauthorizedSubmitter => 1,
            Self::Stale { .. } => 2,
            Self::DeviationExceeded { .. } => 3,
            Self::TrimmedMeanInsufficient { .. } => 4,
            Self::UnknownFeed => 5,
        }
    }
}

impl Encodable for OracleReject {
    fn rlp_append(&self, s: &mut RlpStream) {
        match self {
            Self::UnauthorizedSubmitter | Self::UnknownFeed => {
                s.begin_list(1);
                s.append(&self.tag());
            }
            Self::NotEnoughSubmissions { got, need }
            | Self::TrimmedMeanInsufficient { got, need } => {
                s.begin_list(3);
                s.append(&self.tag());
                s.append(got);
                s.append(need);
            }
            Self::Stale { age_ms, max_ms } => {
                s.begin_list(3);
                s.append(&self.tag());
                s.append(age_ms);
                s.append(max_ms);
            }
            Self::DeviationExceeded { delta_bps, max_bps } => {
                s.begin_list(3);
                s.append(&self.tag());
                s.append(delta_bps);
                s.append(max_bps);
            }
        }
    }
}

impl Decodable for OracleReject {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        let n = rlp.item_count()?;
        if n == 0 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        let tag: u8 = rlp.val_at(0)?;
        match tag {
            0 if n == 3 => Ok(Self::NotEnoughSubmissions {
                got: rlp.val_at(1)?,
                need: rlp.val_at(2)?,
            }),
            1 if n == 1 => Ok(Self::UnauthorizedSubmitter),
            2 if n == 3 => Ok(Self::Stale {
                age_ms: rlp.val_at(1)?,
                max_ms: rlp.val_at(2)?,
            }),
            3 if n == 3 => Ok(Self::DeviationExceeded {
                delta_bps: rlp.val_at(1)?,
                max_bps: rlp.val_at(2)?,
            }),
            4 if n == 3 => Ok(Self::TrimmedMeanInsufficient {
                got: rlp.val_at(1)?,
                need: rlp.val_at(2)?,
            }),
            5 if n == 1 => Ok(Self::UnknownFeed),
            _ => Err(DecoderError::Custom(
                "OracleReject: unknown tag or arity mismatch",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rlp::{decode, encode};

    fn addr(b: u8) -> Address {
        Address([b; 20])
    }

    fn submitters(n: u8) -> BTreeSet<Address> {
        (1..=n).map(addr).collect()
    }

    fn spec_default() -> OracleFeedSpec {
        OracleFeedSpec {
            id: FeedId::new("ZBX/USD").unwrap(),
            aggregation: AggregationKind::Median,
            decimals: 8,
            min_submissions: 3,
            max_staleness_ms: 30_000,
            max_deviation_bps: 5_000,
            authorized_submitters: submitters(5),
        }
    }

    // --- FeedId ---
    #[test]
    fn feedid_rejects_empty_or_too_long() {
        assert!(FeedId::new("").is_err());
        assert!(FeedId::new("X".repeat(33)).is_err());
    }
    #[test]
    fn feedid_rejects_non_ascii() {
        assert!(FeedId::new("ZBX/€").is_err());
    }
    #[test]
    fn feedid_rlp_round_trip() {
        let f = FeedId::new("ZBX/USD").unwrap();
        let bytes = encode(&f);
        let back: FeedId = decode(&bytes).unwrap();
        assert_eq!(f, back);
    }

    // --- AggregationKind ---
    #[test]
    fn agg_round_trip_all() {
        for c in [
            AggregationKind::Median,
            AggregationKind::TrimmedMean { trim_each_side: 1 },
            AggregationKind::StakeWeightedMean,
        ] {
            let bytes = encode(&c);
            let back: AggregationKind = decode(&bytes).unwrap();
            assert_eq!(c, back);
        }
    }

    // --- OracleSubmission ---
    #[test]
    fn submission_rejects_zero_height() {
        let r = OracleSubmission::new(
            FeedId::new("ZBX/USD").unwrap(),
            addr(1),
            0,
            1,
            100,
        );
        assert!(r.is_err());
    }
    #[test]
    fn submission_rlp_round_trip_full_u128() {
        let s = OracleSubmission::new(
            FeedId::new("ZBX/USD").unwrap(),
            addr(1),
            1_000,
            2_000_000,
            u128::MAX - 7,
        )
        .unwrap();
        let bytes = encode(&s);
        let back: OracleSubmission = decode(&bytes).unwrap();
        assert_eq!(s, back);
    }

    // --- OracleFeedSpec ---
    #[test]
    fn spec_default_validates() {
        spec_default().validate().unwrap();
    }
    #[test]
    fn spec_rejects_min_above_authorized() {
        let mut s = spec_default();
        s.min_submissions = 99;
        assert!(s.validate().is_err());
    }
    #[test]
    fn spec_rejects_zero_staleness() {
        let mut s = spec_default();
        s.max_staleness_ms = 0;
        assert!(s.validate().is_err());
    }
    #[test]
    fn spec_rejects_excessive_deviation() {
        let mut s = spec_default();
        s.max_deviation_bps = 200_000;
        assert!(s.validate().is_err());
    }
    #[test]
    fn spec_trimmed_mean_requires_enough_min_submissions() {
        let mut s = spec_default();
        s.aggregation = AggregationKind::TrimmedMean { trim_each_side: 2 };
        s.min_submissions = 4; // need 5
        assert!(s.validate().is_err());
        s.min_submissions = 5;
        s.validate().unwrap();
    }
    #[test]
    fn spec_rlp_round_trip() {
        let s = spec_default();
        let bytes = encode(&s);
        let back: OracleFeedSpec = decode(&bytes).unwrap();
        assert_eq!(s, back);
    }

    // --- OraclePolicy ---
    #[test]
    fn policy_upsert_and_get() {
        let mut p = OraclePolicy::new();
        let s = spec_default();
        p.upsert(s.clone()).unwrap();
        assert_eq!(p.len(), 1);
        assert_eq!(p.get(&s.id), Some(&s));
    }

    #[test]
    fn policy_rlp_round_trip_canonical_order() {
        let mut p = OraclePolicy::new();
        for name in ["ZBX/USD", "ETH/USD", "BTC/USD"] {
            let mut s = spec_default();
            s.id = FeedId::new(name).unwrap();
            p.upsert(s).unwrap();
        }
        let bytes = encode(&p);
        let back: OraclePolicy = decode(&bytes).unwrap();
        assert_eq!(p, back);
        let names: Vec<&str> = back.feeds().keys().map(|f| f.0.as_str()).collect();
        assert_eq!(names, vec!["BTC/USD", "ETH/USD", "ZBX/USD"]);
    }

    #[test]
    fn policy_decode_rejects_key_value_mismatch() {
        let mut s = RlpStream::new_list(1);
        s.begin_list(2);
        s.append(&FeedId::new("AAA").unwrap());
        let mut spec = spec_default();
        spec.id = FeedId::new("BBB").unwrap();
        s.append(&spec);
        let bytes = s.out();
        let r: Result<OraclePolicy, _> = decode(&bytes);
        assert!(matches!(r, Err(DecoderError::Custom(_))));
    }

    // --- OracleReject ---
    #[test]
    fn reject_round_trip_all_variants() {
        let cases = vec![
            OracleReject::NotEnoughSubmissions { got: 1, need: 3 },
            OracleReject::UnauthorizedSubmitter,
            OracleReject::Stale { age_ms: 99, max_ms: 30 },
            OracleReject::DeviationExceeded {
                delta_bps: 10_000,
                max_bps: 5_000,
            },
            OracleReject::TrimmedMeanInsufficient { got: 2, need: 5 },
            OracleReject::UnknownFeed,
        ];
        for c in cases {
            let bytes = encode(&c);
            let back: OracleReject = decode(&bytes).unwrap();
            assert_eq!(c, back);
        }
    }
}
