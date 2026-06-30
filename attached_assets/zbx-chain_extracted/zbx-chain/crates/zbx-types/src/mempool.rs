//! Mempool protection types — rate-limit policy, fee-priority rule,
//! duplicate-rejection policy, and the canonical `MempoolReject` error.
//!
//! Type-and-codec layer. The actual mempool lives in `zbx-bundler` and
//! consumes these types verbatim.
//!
//! Discipline (matches sibling modules):
//! - `BTreeMap` for canonical RLP. `validate()` runs in BOTH constructor AND
//!   `Decodable::decode`.
//! - `s.append(&inner)` inside `begin_list(N)` — never the naked
//!   `inner.rlp_append(s)`, which silently skips the parent counter.
//! - Newtype `Encodable` impls use `self.inner.rlp_append(s)` for direct
//!   delegation (LESSON #11).
//! - `item_count() != N` field-count gate at top of every `decode`.

use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// RateLimitRule — leaky-bucket policy: `capacity` operations refill at
// `refill_per_block` per block. Saturating arithmetic.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RateLimitRule {
    /// Maximum tokens the bucket may hold (= burst size).
    pub capacity: u32,
    /// Tokens added per block-tick.
    pub refill_per_block: u32,
}

impl RateLimitRule {
    pub fn new(capacity: u32, refill_per_block: u32) -> Result<Self, DecoderError> {
        let r = Self { capacity, refill_per_block };
        r.validate()?;
        Ok(r)
    }

    pub fn validate(&self) -> Result<(), DecoderError> {
        if self.capacity == 0 {
            return Err(DecoderError::Custom("RateLimitRule.capacity must be > 0"));
        }
        if self.refill_per_block == 0 {
            return Err(DecoderError::Custom(
                "RateLimitRule.refill_per_block must be > 0",
            ));
        }
        if self.refill_per_block > self.capacity {
            return Err(DecoderError::Custom(
                "RateLimitRule.refill_per_block must be <= capacity",
            ));
        }
        Ok(())
    }

    /// Compute tokens after `n_blocks` of refill, starting from `current`.
    pub fn refill(&self, current: u32, n_blocks: u32) -> u32 {
        current
            .saturating_add(self.refill_per_block.saturating_mul(n_blocks))
            .min(self.capacity)
    }
}

impl Encodable for RateLimitRule {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(2);
        s.append(&self.capacity);
        s.append(&self.refill_per_block);
    }
}

impl Decodable for RateLimitRule {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if rlp.item_count()? != 2 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        let r = Self {
            capacity: rlp.val_at(0)?,
            refill_per_block: rlp.val_at(1)?,
        };
        r.validate()?;
        Ok(r)
    }
}

// ---------------------------------------------------------------------------
// PriorityRule — fee-density and tie-break ordering.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PriorityRule {
    /// Minimum gas price (in wei-equivalent base units) accepted.
    pub min_gas_price: u64,
    /// Minimum tip (priority fee) accepted on top of base fee.
    pub min_priority_tip: u64,
    /// When two txs have identical fee-density, the older-by-arrival wins
    /// iff `prefer_first_seen` is true; otherwise canonical order is by
    /// `(sender, nonce)` ascending.
    pub prefer_first_seen: bool,
}

impl PriorityRule {
    pub fn mainnet_default() -> Self {
        Self {
            min_gas_price: 1_000_000_000, // 1 gwei
            min_priority_tip: 1,
            prefer_first_seen: true,
        }
    }

    pub fn testnet_default() -> Self {
        Self {
            min_gas_price: 1,
            min_priority_tip: 0,
            prefer_first_seen: true,
        }
    }

    pub fn validate(&self) -> Result<(), DecoderError> {
        if self.min_gas_price == 0 {
            return Err(DecoderError::Custom("min_gas_price must be > 0"));
        }
        Ok(())
    }
}

impl Encodable for PriorityRule {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(3);
        s.append(&self.min_gas_price);
        s.append(&self.min_priority_tip);
        s.append(&self.prefer_first_seen);
    }
}

impl Decodable for PriorityRule {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if rlp.item_count()? != 3 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        let r = Self {
            min_gas_price: rlp.val_at(0)?,
            min_priority_tip: rlp.val_at(1)?,
            prefer_first_seen: rlp.val_at(2)?,
        };
        r.validate()?;
        Ok(r)
    }
}

// ---------------------------------------------------------------------------
// MempoolPolicy — full per-network mempool admission/eviction rules.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MempoolPolicy {
    /// Hard cap on total entries in the mempool.
    pub max_pool_size: u32,
    /// Per-account in-flight tx cap (queued + pending).
    pub max_per_account: u32,
    /// Per-IP rate-limit rule.
    pub per_ip_rate_limit: RateLimitRule,
    /// Per-account rate-limit rule.
    pub per_account_rate_limit: RateLimitRule,
    /// Fee priority + tie-break.
    pub priority: PriorityRule,
    /// Drop a tx as duplicate iff its (sender, nonce, hash) was seen within
    /// this many blocks. Zero disables duplicate-window detection.
    pub duplicate_window_blocks: u32,
    /// Maximum queued entries before mempool starts evicting lowest-fee.
    pub eviction_high_water: u32,
}

impl MempoolPolicy {
    pub fn mainnet_default() -> Self {
        Self {
            max_pool_size: 5_000,
            max_per_account: 64,
            per_ip_rate_limit: RateLimitRule::new(100, 10).expect("valid"),
            per_account_rate_limit: RateLimitRule::new(50, 5).expect("valid"),
            priority: PriorityRule::mainnet_default(),
            duplicate_window_blocks: 256,
            eviction_high_water: 4_500,
        }
    }

    pub fn testnet_default() -> Self {
        Self {
            max_pool_size: 20_000,
            max_per_account: 256,
            per_ip_rate_limit: RateLimitRule::new(1_000, 100).expect("valid"),
            per_account_rate_limit: RateLimitRule::new(500, 50).expect("valid"),
            priority: PriorityRule::testnet_default(),
            duplicate_window_blocks: 64,
            eviction_high_water: 18_000,
        }
    }

    pub fn validate(&self) -> Result<(), DecoderError> {
        if self.max_pool_size == 0 {
            return Err(DecoderError::Custom("max_pool_size must be > 0"));
        }
        if self.max_per_account == 0 {
            return Err(DecoderError::Custom("max_per_account must be > 0"));
        }
        if self.eviction_high_water == 0
            || self.eviction_high_water > self.max_pool_size
        {
            return Err(DecoderError::Custom(
                "eviction_high_water must be > 0 and <= max_pool_size",
            ));
        }
        self.per_ip_rate_limit.validate()?;
        self.per_account_rate_limit.validate()?;
        self.priority.validate()?;
        Ok(())
    }
}

impl Encodable for MempoolPolicy {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(7);
        s.append(&self.max_pool_size);
        s.append(&self.max_per_account);
        s.append(&self.per_ip_rate_limit);
        s.append(&self.per_account_rate_limit);
        s.append(&self.priority);
        s.append(&self.duplicate_window_blocks);
        s.append(&self.eviction_high_water);
    }
}

impl Decodable for MempoolPolicy {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if rlp.item_count()? != 7 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        let p = Self {
            max_pool_size: rlp.val_at(0)?,
            max_per_account: rlp.val_at(1)?,
            per_ip_rate_limit: rlp.val_at(2)?,
            per_account_rate_limit: rlp.val_at(3)?,
            priority: rlp.val_at(4)?,
            duplicate_window_blocks: rlp.val_at(5)?,
            eviction_high_water: rlp.val_at(6)?,
        };
        p.validate()?;
        Ok(p)
    }
}

// ---------------------------------------------------------------------------
// MempoolReject — canonical reject reason (returned by `add(tx)`).
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum MempoolReject {
    /// Pool is full and tx fee is below current eviction floor.
    PoolFull,
    /// Sender exceeded per-account in-flight cap.
    AccountCapExceeded { current: u32, max: u32 },
    /// IP exceeded rate-limit bucket.
    IpRateLimited { tokens_left: u32 },
    /// Account exceeded rate-limit bucket.
    AccountRateLimited { tokens_left: u32 },
    /// Tx fee below `min_gas_price` or tip below `min_priority_tip`.
    FeeTooLow { provided: u64, required: u64 },
    /// Identical tx hash seen within duplicate-window.
    Duplicate,
    /// Underlying validation failed (signature, nonce, size — see
    /// `ValidationError`); discriminant carried as opaque tag.
    ValidationFailed { tag: u8 },
}

impl MempoolReject {
    pub fn tag(&self) -> u8 {
        match self {
            Self::PoolFull => 0,
            Self::AccountCapExceeded { .. } => 1,
            Self::IpRateLimited { .. } => 2,
            Self::AccountRateLimited { .. } => 3,
            Self::FeeTooLow { .. } => 4,
            Self::Duplicate => 5,
            Self::ValidationFailed { .. } => 6,
        }
    }
}

impl Encodable for MempoolReject {
    fn rlp_append(&self, s: &mut RlpStream) {
        match self {
            Self::PoolFull | Self::Duplicate => {
                s.begin_list(1);
                s.append(&self.tag());
            }
            Self::AccountCapExceeded { current, max } => {
                s.begin_list(3);
                s.append(&self.tag());
                s.append(current);
                s.append(max);
            }
            Self::IpRateLimited { tokens_left } => {
                s.begin_list(2);
                s.append(&self.tag());
                s.append(tokens_left);
            }
            Self::AccountRateLimited { tokens_left } => {
                s.begin_list(2);
                s.append(&self.tag());
                s.append(tokens_left);
            }
            Self::FeeTooLow { provided, required } => {
                s.begin_list(3);
                s.append(&self.tag());
                s.append(provided);
                s.append(required);
            }
            Self::ValidationFailed { tag } => {
                s.begin_list(2);
                s.append(&self.tag());
                s.append(tag);
            }
        }
    }
}

impl Decodable for MempoolReject {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        let n = rlp.item_count()?;
        if n == 0 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        let tag: u8 = rlp.val_at(0)?;
        match tag {
            0 if n == 1 => Ok(Self::PoolFull),
            1 if n == 3 => Ok(Self::AccountCapExceeded {
                current: rlp.val_at(1)?,
                max: rlp.val_at(2)?,
            }),
            2 if n == 2 => Ok(Self::IpRateLimited {
                tokens_left: rlp.val_at(1)?,
            }),
            3 if n == 2 => Ok(Self::AccountRateLimited {
                tokens_left: rlp.val_at(1)?,
            }),
            4 if n == 3 => Ok(Self::FeeTooLow {
                provided: rlp.val_at(1)?,
                required: rlp.val_at(2)?,
            }),
            5 if n == 1 => Ok(Self::Duplicate),
            6 if n == 2 => Ok(Self::ValidationFailed {
                tag: rlp.val_at(1)?,
            }),
            _ => Err(DecoderError::Custom(
                "MempoolReject: unknown tag or arity mismatch",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rlp::{decode, encode};

    // --- RateLimitRule ---
    #[test]
    fn rate_rejects_zero() {
        assert!(RateLimitRule::new(0, 1).is_err());
        assert!(RateLimitRule::new(10, 0).is_err());
    }
    #[test]
    fn rate_rejects_refill_above_capacity() {
        assert!(RateLimitRule::new(5, 10).is_err());
    }
    #[test]
    fn rate_refill_caps_at_capacity() {
        let r = RateLimitRule::new(100, 10).unwrap();
        assert_eq!(r.refill(50, 100), 100);
    }
    #[test]
    fn rate_rlp_round_trip() {
        let r = RateLimitRule::new(100, 10).unwrap();
        let bytes = encode(&r);
        let back: RateLimitRule = decode(&bytes).unwrap();
        assert_eq!(r, back);
    }

    // --- PriorityRule ---
    #[test]
    fn prio_rejects_zero_min_gas() {
        let mut p = PriorityRule::mainnet_default();
        p.min_gas_price = 0;
        assert!(p.validate().is_err());
    }
    #[test]
    fn prio_rlp_round_trip() {
        let p = PriorityRule::mainnet_default();
        let bytes = encode(&p);
        let back: PriorityRule = decode(&bytes).unwrap();
        assert_eq!(p, back);
    }

    // --- MempoolPolicy ---
    #[test]
    fn policy_mainnet_validates() {
        MempoolPolicy::mainnet_default().validate().unwrap();
    }
    #[test]
    fn policy_testnet_validates() {
        MempoolPolicy::testnet_default().validate().unwrap();
    }
    #[test]
    fn policy_rejects_eviction_above_pool_size() {
        let mut p = MempoolPolicy::mainnet_default();
        p.eviction_high_water = p.max_pool_size + 1;
        assert!(p.validate().is_err());
    }
    #[test]
    fn policy_rejects_zero_max_pool() {
        let mut p = MempoolPolicy::mainnet_default();
        p.max_pool_size = 0;
        assert!(p.validate().is_err());
    }
    #[test]
    fn policy_rlp_round_trip() {
        let p = MempoolPolicy::mainnet_default();
        let bytes = encode(&p);
        let back: MempoolPolicy = decode(&bytes).unwrap();
        assert_eq!(p, back);
    }
    #[test]
    fn policy_decode_rejects_wrong_field_count() {
        let mut s = RlpStream::new_list(6);
        s.append(&1u32);
        s.append(&1u32);
        s.append(&RateLimitRule::new(10, 1).unwrap());
        s.append(&RateLimitRule::new(10, 1).unwrap());
        s.append(&PriorityRule::mainnet_default());
        s.append(&1u32);
        let bytes = s.out();
        let r: Result<MempoolPolicy, _> = decode(&bytes);
        assert!(matches!(r, Err(DecoderError::RlpIncorrectListLen)));
    }

    // --- MempoolReject ---
    #[test]
    fn reject_round_trip_all_variants() {
        let cases = vec![
            MempoolReject::PoolFull,
            MempoolReject::AccountCapExceeded { current: 10, max: 5 },
            MempoolReject::IpRateLimited { tokens_left: 0 },
            MempoolReject::AccountRateLimited { tokens_left: 0 },
            MempoolReject::FeeTooLow { provided: 1, required: 2 },
            MempoolReject::Duplicate,
            MempoolReject::ValidationFailed { tag: 7 },
        ];
        for c in cases {
            let bytes = encode(&c);
            let back: MempoolReject = decode(&bytes).unwrap();
            assert_eq!(c, back);
        }
    }
    #[test]
    fn reject_decode_rejects_unknown_tag() {
        let mut s = RlpStream::new_list(1);
        s.append(&99u8);
        let bytes = s.out();
        let r: Result<MempoolReject, _> = decode(&bytes);
        assert!(r.is_err());
    }
}
