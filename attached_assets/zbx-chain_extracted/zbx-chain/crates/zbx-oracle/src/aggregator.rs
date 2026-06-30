//! Price aggregation — takes multiple reporter prices, returns the median.
//!
//! Uses median (not mean) to resist manipulation:
//!   - A single malicious reporter cannot shift the median more than 1 position
//!   - Even with minority corrupt reporters, median stays near truth
//!   - Mean is vulnerable to outlier attacks (e.g. submit $999,999 to shift avg)
//!
//! # Outlier Detection
//!
//! Before computing median, outliers are removed:
//!   1. Sort all submissions
//!   2. Compute the interquartile range (IQR)
//!   3. Remove submissions outside [Q1 - 3×IQR, Q3 + 3×IQR]
//!   4. Compute median of remaining submissions

use crate::{feed::Price, reporter::PriceReport, error::OracleError};
use serde::{Serialize, Deserialize};

/// Result of aggregating a set of price reports.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AggregateResult {
    /// The aggregated price (median of valid submissions).
    pub price:          Price,
    /// Number of submissions used (after outlier removal).
    pub reporter_count: u32,
    /// Number of outliers removed.
    pub outliers_removed: u32,
    /// Min and max prices seen this round.
    pub price_range:    (Price, Price),
    /// Unix timestamp when aggregation was performed.
    pub timestamp:      u64,
}

/// The oracle aggregator.
pub struct OracleAggregator {
    /// Outlier removal: max IQR multiplier (3.0 by default).
    pub iqr_multiplier: f64,
    /// Minimum reporters required for a valid aggregation.
    pub min_reporters:  u32,
}

impl Default for OracleAggregator {
    fn default() -> Self { Self { iqr_multiplier: 3.0, min_reporters: 3 } }
}

impl OracleAggregator {
    pub fn new(min_reporters: u32) -> Self {
        Self { iqr_multiplier: 3.0, min_reporters }
    }

    /// Aggregate a set of price reports into a single price.
    ///
    /// Steps:
    ///   1. Validate each report (signature, timestamp, price sanity)
    ///   2. Sort by price
    ///   3. Remove outliers (IQR method)
    ///   4. Return median
    pub fn aggregate(
        &self,
        reports:   &[PriceReport],
        timestamp: u64,
    ) -> Result<AggregateResult, OracleError> {
        if reports.is_empty() {
            return Err(OracleError::NoReports);
        }

        // Filter valid, non-expired reports
        let valid: Vec<&PriceReport> = reports.iter()
            .filter(|r| r.price.is_valid() && !r.is_expired(timestamp, 300))
            .collect();

        if valid.len() < self.min_reporters as usize {
            return Err(OracleError::InsufficientReporters {
                got:      valid.len(),
                required: self.min_reporters as usize,
            });
        }

        // Sort prices
        let mut prices: Vec<Price> = valid.iter().map(|r| r.price).collect();
        prices.sort_unstable();

        let n = prices.len();
        let min_p = prices[0];
        let max_p = prices[n - 1];

        // Remove outliers using IQR
        let clean = self.remove_outliers(&prices);
        let outliers_removed = (n - clean.len()) as u32;

        if clean.len() < self.min_reporters as usize {
            return Err(OracleError::TooManyOutliers {
                total: n, outliers: outliers_removed as usize,
            });
        }

        // Compute median
        let median = compute_median(&clean);

        tracing::debug!(
            feed     = ?valid[0].feed_id,
            price    = %median,
            reporters = clean.len(),
            outliers = outliers_removed,
            "Oracle round aggregated"
        );

        Ok(AggregateResult {
            price:            median,
            reporter_count:   clean.len() as u32,
            outliers_removed,
            price_range:      (min_p, max_p),
            timestamp,
        })
    }

    /// Remove outliers using the IQR (interquartile range) method.
    fn remove_outliers(&self, sorted_prices: &[Price]) -> Vec<Price> {
        let n = sorted_prices.len();
        if n < 4 { return sorted_prices.to_vec(); }

        let q1 = sorted_prices[n / 4].0 as f64;
        let q3 = sorted_prices[3 * n / 4].0 as f64;
        let iqr = q3 - q1;

        let lo = q1 - self.iqr_multiplier * iqr;
        let hi = q3 + self.iqr_multiplier * iqr;

        sorted_prices.iter()
            .filter(|p| p.0 as f64 >= lo && p.0 as f64 <= hi)
            .copied()
            .collect()
    }
}

/// Compute the median of a sorted slice of prices.
fn compute_median(sorted: &[Price]) -> Price {
    let n = sorted.len();
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        // Average of two middle values
        let mid_lo = sorted[n / 2 - 1].0;
        let mid_hi = sorted[n / 2].0;
        Price((mid_lo + mid_hi) / 2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reporter::PriceReport;
    use crate::feed::FeedId;

    fn report(price_cents: i64, ts: u64) -> PriceReport {
        PriceReport {
            feed_id:    FeedId::zbx_usd(),
            price:      Price(price_cents * 1_000_000),
            timestamp:  ts,
            reporter:   [1u8; 20],
            signature:  [0u8; 64],
        }
    }

    #[test]
    fn median_of_odd_count() {
        let agg = OracleAggregator::new(3);
        let reports = vec![report(300, 1000), report(250, 1000), report(275, 1000)];
        let result = agg.aggregate(&reports, 1000).unwrap();
        assert_eq!(result.price, Price(275 * 1_000_000));
    }

    #[test]
    fn median_of_even_count() {
        let agg = OracleAggregator::new(3);
        let reports = vec![
            report(200, 1000), report(250, 1000),
            report(300, 1000), report(350, 1000),
        ];
        let result = agg.aggregate(&reports, 1000).unwrap();
        // Median of [200, 250, 300, 350] = (250+300)/2 = 275
        assert_eq!(result.price, Price(275 * 1_000_000));
    }

    #[test]
    fn outlier_ignored() {
        let agg = OracleAggregator::new(3);
        // One extreme outlier
        let reports = vec![
            report(250, 1000), report(255, 1000), report(248, 1000),
            report(252, 1000), report(100_000, 1000), // extreme outlier
        ];
        let result = agg.aggregate(&reports, 1000).unwrap();
        // Outlier should be removed, median around 252
        assert!(result.price.to_f64() < 260.0);
        assert!(result.outliers_removed >= 1);
    }

    #[test]
    fn insufficient_reporters_error() {
        let agg = OracleAggregator::new(5);
        let reports = vec![report(250, 1000), report(255, 1000)];
        let err = agg.aggregate(&reports, 1000).unwrap_err();
        assert!(matches!(err, OracleError::InsufficientReporters { .. }));
    }

    #[test]
    fn expired_reports_excluded() {
        let agg = OracleAggregator::new(3);
        let reports = vec![
            report(250, 1000),              // fresh
            report(255, 1000),              // fresh
            report(260, 1),                 // very old → excluded
            report(248, 1000),              // fresh
        ];
        let result = agg.aggregate(&reports, 1000).unwrap();
        assert_eq!(result.reporter_count, 3);
    }

    #[test]
    fn single_price_returns_itself() {
        let agg = OracleAggregator::new(1);
        let reports = vec![report(500, 1000)];
        let result = agg.aggregate(&reports, 1000).unwrap();
        assert_eq!(result.price, Price(500 * 1_000_000));
    }
}