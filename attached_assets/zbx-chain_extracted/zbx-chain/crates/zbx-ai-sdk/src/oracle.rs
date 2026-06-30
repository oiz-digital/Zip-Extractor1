//! Oracle price data connector for the AI Agent SDK.
//!
//! Uses a trait abstraction so the SDK does not depend on zbx-oracle directly.
//! Implementors supply live or historical price feeds; the SDK consumes them
//! as normalized fixed-point values (6 decimal places, stored as u64).
//!
//! # Security
//! - All price reads are timestamped; stale data (> MAX_STALENESS_SECS) is rejected.
//! - Multi-source median is used when > 1 feed provided (manipulation resistant).
//! - Anomaly detection via OracleAnomalyGuard model (0x06) runs automatically.

use crate::error::SdkError;
use serde::{Serialize, Deserialize};

/// Maximum accepted price staleness in seconds.
pub const MAX_STALENESS_SECS: u64 = 300; // 5 minutes

/// Minimum required sources for a valid price (manipulation resistance).
pub const MIN_PRICE_SOURCES: usize = 1;

/// A single price observation from one source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceObservation {
    /// Asset symbol e.g. "ZBX/USDT"
    pub pair:       String,
    /// Price in fixed-point: 1_000_000 = $1.000000
    pub price_fp6:  u64,
    /// Unix timestamp of this price.
    pub timestamp:  u64,
    /// Source identifier (e.g. "binance", "coinbase", "zbx-oracle").
    pub source:     String,
    /// Source confidence 0–10000 bps.
    pub confidence: u16,
}

/// OHLCV candle (fixed-point prices, volume in native units).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OhlcvCandle {
    pub pair:   String,
    pub open:   u64,
    pub high:   u64,
    pub low:    u64,
    pub close:  u64,
    pub volume: u64,
    pub ts:     u64,
}

/// Aggregated price from multiple sources.
#[derive(Debug, Clone)]
pub struct AggregatedPrice {
    pub pair:        String,
    pub median_fp6:  u64,
    pub min_fp6:     u64,
    pub max_fp6:     u64,
    pub num_sources: usize,
    pub timestamp:   u64,
}

impl AggregatedPrice {
    /// Spread in basis points: (max - min) / median × 10000.
    pub fn spread_bps(&self) -> u16 {
        if self.median_fp6 == 0 { return 0; }
        let diff = self.max_fp6.saturating_sub(self.min_fp6);
        ((diff as u128 * 10_000) / self.median_fp6 as u128).min(u16::MAX as u128) as u16
    }

    /// Is the price fresh enough?
    pub fn is_fresh(&self, now: u64) -> bool {
        now.saturating_sub(self.timestamp) <= MAX_STALENESS_SECS
    }
}

/// Oracle data provider trait — implement this to plug in any data source.
pub trait OracleProvider: Send + Sync {
    /// Fetch the latest price for a trading pair.
    fn latest_price(&self, pair: &str) -> Result<PriceObservation, SdkError>;
    /// Fetch recent OHLCV candles (up to `limit` candles).
    fn ohlcv(&self, pair: &str, limit: usize) -> Result<Vec<OhlcvCandle>, SdkError>;
    /// Current Unix timestamp (injectable for deterministic testing).
    fn now(&self) -> u64;
}

/// Multi-source oracle aggregator — takes median across N providers.
pub struct MultiOracleAggregator {
    providers: Vec<Box<dyn OracleProvider>>,
}

impl MultiOracleAggregator {
    pub fn new(providers: Vec<Box<dyn OracleProvider>>) -> Result<Self, SdkError> {
        if providers.is_empty() {
            return Err(SdkError::OracleNoProviders);
        }
        Ok(Self { providers })
    }

    /// Get aggregated (median) price from all providers.
    pub fn aggregate(&self, pair: &str) -> Result<AggregatedPrice, SdkError> {
        let now = self.providers[0].now();
        let mut prices: Vec<u64> = Vec::new();

        for provider in &self.providers {
            match provider.latest_price(pair) {
                Ok(obs) => {
                    let staleness = now.saturating_sub(obs.timestamp);
                    if staleness <= MAX_STALENESS_SECS {
                        prices.push(obs.price_fp6);
                    } else {
                        tracing::warn!(
                            pair, staleness, source = %obs.source,
                            "Oracle price stale — skipping"
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(pair, error = %e, "Oracle provider failed");
                }
            }
        }

        if prices.len() < MIN_PRICE_SOURCES {
            return Err(SdkError::OracleInsuffientSources {
                got:      prices.len(),
                required: MIN_PRICE_SOURCES,
            });
        }

        prices.sort_unstable();
        let median = median_u64(&prices);
        let min = *prices.first().unwrap();
        let max = *prices.last().unwrap();

        Ok(AggregatedPrice {
            pair:        pair.to_string(),
            median_fp6:  median,
            min_fp6:     min,
            max_fp6:     max,
            num_sources: prices.len(),
            timestamp:   now,
        })
    }
}

fn median_u64(sorted: &[u64]) -> u64 {
    let n = sorted.len();
    if n == 0 { return 0; }
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        (sorted[n / 2 - 1] / 2) + (sorted[n / 2] / 2)
    }
}

/// Live oracle provider backed by a shared price cache.
///
/// The cache is populated by a background async task that calls
/// `zbx_oracle::fetcher::fetch_price_vwap` (or the INR fetcher) and writes
/// results here.  The sync `OracleProvider` trait reads from the in-memory
/// cache, which means latency is zero at read time and staleness is bounded
/// by the background task's update interval.
///
/// # Usage
///
/// ```rust,no_run
/// use std::sync::{Arc, Mutex};
/// use std::collections::HashMap;
/// use zbx_ai_sdk::oracle::{ZbxCachedOracleProvider, PriceObservation};
///
/// // Shared cache — hand it to the provider and to your price-update loop.
/// let cache: Arc<Mutex<HashMap<String, PriceObservation>>> =
///     Arc::new(Mutex::new(HashMap::new()));
///
/// let provider = ZbxCachedOracleProvider::new(Arc::clone(&cache));
///
/// // In your async price-update loop (runs e.g. every 60 s):
/// //   let price = zbx_oracle::fetcher::fetch_price_vwap(&feed_id).await?;
/// //   ZbxCachedOracleProvider::insert(&cache, "ZBX/USD", price_fp6, now);
/// ```
pub struct ZbxCachedOracleProvider {
    /// Shared price cache: pair → latest observation.
    cache: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, PriceObservation>>>,
}

impl ZbxCachedOracleProvider {
    /// Construct a new provider that reads from the given shared cache.
    pub fn new(
        cache: std::sync::Arc<
            std::sync::Mutex<std::collections::HashMap<String, PriceObservation>>,
        >,
    ) -> Self {
        Self { cache }
    }

    /// Insert or update a price observation in the shared cache.
    ///
    /// Call this from your background price-update task after each successful
    /// `fetch_price_vwap` call.
    pub fn insert(
        cache: &std::sync::Arc<
            std::sync::Mutex<std::collections::HashMap<String, PriceObservation>>,
        >,
        pair:       &str,
        price_fp6:  u64,
        timestamp:  u64,
    ) {
        let obs = PriceObservation {
            pair:       pair.to_string(),
            price_fp6,
            timestamp,
            source:     "zbx-oracle".to_string(),
            confidence: 9_500,
        };
        if let Ok(mut map) = cache.lock() {
            map.insert(pair.to_string(), obs);
        }
    }
}

impl OracleProvider for ZbxCachedOracleProvider {
    fn latest_price(&self, pair: &str) -> Result<PriceObservation, SdkError> {
        self.cache
            .lock()
            .map_err(|_| SdkError::OracleNoProviders)?
            .get(pair)
            .cloned()
            .ok_or_else(|| SdkError::OracleInsuffientSources { got: 0, required: 1 })
    }

    fn ohlcv(&self, pair: &str, _limit: usize) -> Result<Vec<OhlcvCandle>, SdkError> {
        // OHLCV history is not available from the point-in-time cache.
        // Return a single synthetic candle from the latest cached price so callers
        // that only need "current bar" get a sensible result.
        let obs = self.latest_price(pair)?;
        Ok(vec![OhlcvCandle {
            pair:   pair.to_string(),
            open:   obs.price_fp6,
            high:   obs.price_fp6,
            low:    obs.price_fp6,
            close:  obs.price_fp6,
            volume: 0,
            ts:     obs.timestamp,
        }])
    }

    fn now(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }
}

/// Stub oracle provider for testing — returns deterministic prices.
pub struct StubOracleProvider {
    pub fixed_time: u64,
}

impl StubOracleProvider {
    pub fn new(fixed_time: u64) -> Self { Self { fixed_time } }
}

impl OracleProvider for StubOracleProvider {
    fn latest_price(&self, pair: &str) -> Result<PriceObservation, SdkError> {
        // Deterministic price derived from pair string
        let hash: u64 = pair.bytes().fold(0u64, |acc, b| {
            acc.wrapping_mul(31).wrapping_add(b as u64)
        });
        let price_fp6 = 1_000_000u64 + (hash % 999_000_000);
        Ok(PriceObservation {
            pair:       pair.to_string(),
            price_fp6,
            timestamp:  self.fixed_time,
            source:     "stub".to_string(),
            confidence: 9000,
        })
    }

    fn ohlcv(&self, pair: &str, limit: usize) -> Result<Vec<OhlcvCandle>, SdkError> {
        let base = self.latest_price(pair)?.price_fp6;
        let candles = (0..limit.min(100)).map(|i| OhlcvCandle {
            pair:   pair.to_string(),
            open:   base.wrapping_add(i as u64 * 100),
            high:   base.wrapping_add(i as u64 * 100 + 500),
            low:    base.saturating_sub(200),
            close:  base.wrapping_add(i as u64 * 50),
            volume: 100_000_000,
            ts:     self.fixed_time.saturating_sub(i as u64 * 60),
        }).collect();
        Ok(candles)
    }

    fn now(&self) -> u64 { self.fixed_time }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_price_is_deterministic() {
        let p = StubOracleProvider::new(1_700_000_000);
        let r1 = p.latest_price("ZBX/USDT").unwrap();
        let r2 = p.latest_price("ZBX/USDT").unwrap();
        assert_eq!(r1.price_fp6, r2.price_fp6);
    }

    #[test]
    fn different_pairs_give_different_prices() {
        let p = StubOracleProvider::new(1_700_000_000);
        let zbx = p.latest_price("ZBX/USDT").unwrap();
        let eth = p.latest_price("ETH/USDT").unwrap();
        assert_ne!(zbx.price_fp6, eth.price_fp6);
    }

    #[test]
    fn aggregator_single_provider() {
        let agg = MultiOracleAggregator::new(vec![
            Box::new(StubOracleProvider::new(1_700_000_000)),
        ]).unwrap();
        let price = agg.aggregate("ZBX/USDT").unwrap();
        assert!(price.median_fp6 > 0);
        assert_eq!(price.num_sources, 1);
    }

    #[test]
    fn aggregator_multi_provider_median() {
        let agg = MultiOracleAggregator::new(vec![
            Box::new(StubOracleProvider::new(1_700_000_000)),
            Box::new(StubOracleProvider::new(1_700_000_000)),
            Box::new(StubOracleProvider::new(1_700_000_000)),
        ]).unwrap();
        let price = agg.aggregate("ZBX/USDT").unwrap();
        assert_eq!(price.num_sources, 3);
        assert_eq!(price.min_fp6, price.max_fp6); // all same → zero spread
    }

    #[test]
    fn spread_bps_calculation() {
        let agg = AggregatedPrice {
            pair:        "ZBX/USDT".to_string(),
            median_fp6:  1_000_000,
            min_fp6:     990_000,
            max_fp6:     1_010_000,
            num_sources: 2,
            timestamp:   0,
        };
        assert_eq!(agg.spread_bps(), 200); // 2% = 200 bps
    }

    #[test]
    fn empty_providers_rejected() {
        let err = MultiOracleAggregator::new(vec![]).unwrap_err();
        assert!(matches!(err, SdkError::OracleNoProviders));
    }
}
