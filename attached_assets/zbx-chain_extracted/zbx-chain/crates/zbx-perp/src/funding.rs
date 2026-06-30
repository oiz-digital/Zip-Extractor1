//! 8-hour funding rate engine.
//!
//! Funding rate is proportional to the OI imbalance between longs and shorts.
//! It settles every `FUNDING_INTERVAL` (8 hours). Longs pay shorts when
//! long OI > short OI, and vice-versa.
//!
//! Formula (per-interval rate in FUNDING_RATE_SCALE units):
//!   long_bias_bps  = (total_long_oi × 10000) / total_oi
//!   short_bias_bps = (total_short_oi × 10000) / total_oi
//!   rate = (long_bias_bps − short_bias_bps) × FUNDING_RATE_SCALE / 1_000_000
//!
//! A positive rate means longs pay shorts; negative means shorts pay longs.

use crate::types::Market;

/// Funding rate scale factor (mirrors the Solidity constant).
pub const FUNDING_RATE_SCALE: i128 = 10_000_000_000; // 1e10

/// Compute the current per-interval funding rate for a market (in FUNDING_RATE_SCALE units).
/// Returns 0 if total OI is zero.
pub fn current_funding_rate(market: &Market) -> i128 {
    let total_oi = market.total_long_oi + market.total_short_oi;
    if total_oi == 0 {
        return 0;
    }
    let long_bias  = ((market.total_long_oi as i128) * 10_000) / (total_oi as i128);
    let short_bias = ((market.total_short_oi as i128) * 10_000) / (total_oi as i128);
    (long_bias - short_bias) * FUNDING_RATE_SCALE / 1_000_000
}

/// Settle funding for a market, advancing `cumulative_funding` by the correct
/// number of intervals. Returns the number of intervals settled (0 if not due).
///
/// This mirrors `_updateFunding()` in ZbxPerpetuals.sol.
pub fn settle_funding(market: &mut Market, now: u64, funding_interval: u64) -> u64 {
    let elapsed = now.saturating_sub(market.last_funding_update);
    if elapsed < funding_interval {
        return 0;
    }
    let intervals = elapsed / funding_interval;
    market.last_funding_update += intervals * funding_interval;

    let rate = current_funding_rate(market);
    market.cumulative_funding = market.cumulative_funding
        .saturating_add(rate.saturating_mul(intervals as i128));

    intervals
}

/// Seconds until the next scheduled funding settlement for a market.
/// Returns 0 if funding is already overdue.
pub fn next_funding_in(market: &Market, now: u64, funding_interval: u64) -> u64 {
    let next = market.last_funding_update.saturating_add(funding_interval);
    if now >= next { 0 } else { next - now }
}

/// Funding cost accrued for a position since it was opened (signed).
/// Positive = position owes funding; negative = position earns funding.
///
/// Formula: (cumulative_now − funding_entry_rate) × size / FUNDING_RATE_SCALE
pub fn funding_cost_for_position(
    cumulative_now: i128,
    funding_entry_rate: i128,
    size: u128,
) -> i128 {
    let delta = cumulative_now.saturating_sub(funding_entry_rate);
    delta.saturating_mul(size as i128)
        .checked_div(FUNDING_RATE_SCALE)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Market;
    use zbx_types::address::Address;

    fn dummy_market(long_oi: u128, short_oi: u128) -> Market {
        Market {
            symbol: "BTC".into(),
            oracle: Address([0u8; 20]),
            active: true,
            max_leverage: 200,
            total_long_oi: long_oi,
            total_short_oi: short_oi,
            cumulative_funding: 0,
            last_funding_update: 1_000_000,
        }
    }

    #[test]
    fn balanced_oi_gives_zero_rate() {
        let m = dummy_market(1_000, 1_000);
        assert_eq!(current_funding_rate(&m), 0);
    }

    #[test]
    fn long_dominated_oi_gives_positive_rate() {
        let m = dummy_market(7_500, 2_500);
        let rate = current_funding_rate(&m);
        assert!(rate > 0, "expected positive rate for long-dominated OI");
    }

    #[test]
    fn short_dominated_oi_gives_negative_rate() {
        let m = dummy_market(2_500, 7_500);
        let rate = current_funding_rate(&m);
        assert!(rate < 0, "expected negative rate for short-dominated OI");
    }

    #[test]
    fn zero_oi_gives_zero_rate() {
        let m = dummy_market(0, 0);
        assert_eq!(current_funding_rate(&m), 0);
    }

    #[test]
    fn settle_advances_cumulative_funding() {
        let interval = 28_800u64; // 8 hours
        let mut m = dummy_market(6_000, 4_000);
        let rate_before = current_funding_rate(&m);
        let intervals = settle_funding(&mut m, 1_000_000 + interval * 3 + 100, interval);
        assert_eq!(intervals, 3);
        assert_eq!(m.cumulative_funding, rate_before * 3);
        assert_eq!(m.last_funding_update, 1_000_000 + interval * 3);
    }

    #[test]
    fn settle_noop_if_not_due() {
        let interval = 28_800u64;
        let mut m = dummy_market(6_000, 4_000);
        let n = settle_funding(&mut m, 1_000_000 + interval / 2, interval);
        assert_eq!(n, 0);
        assert_eq!(m.cumulative_funding, 0);
    }

    #[test]
    fn next_funding_in_correct() {
        let interval = 28_800u64;
        let m = dummy_market(0, 0);
        let now = 1_000_000 + 10_000;
        let expected = interval - 10_000;
        assert_eq!(next_funding_in(&m, now, interval), expected);
    }

    #[test]
    fn funding_cost_positive_when_cumulative_increases() {
        let cost = funding_cost_for_position(1_000_000, 0, FUNDING_RATE_SCALE as u128 * 2);
        assert!(cost > 0);
    }

    #[test]
    fn funding_cost_zero_when_no_delta() {
        let cost = funding_cost_for_position(500, 500, 1_000_000);
        assert_eq!(cost, 0);
    }
}
