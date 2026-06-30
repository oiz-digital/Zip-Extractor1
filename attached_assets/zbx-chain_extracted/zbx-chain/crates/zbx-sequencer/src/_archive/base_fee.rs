//! EIP-1559 base fee computation for ZBX chain.
//!
//! Algorithm (identical to Ethereum post-London):
//!
//!   gas_target = block_gas_limit / 2
//!
//!   if gas_used == gas_target:
//!     base_fee unchanged
//!
//!   if gas_used > gas_target:        // block too full → fee rises
//!     delta = base_fee × (gas_used - gas_target) / gas_target / 8
//!     next  = base_fee + max(delta, 1)   (at least +1 wei)
//!
//!   if gas_used < gas_target:        // block too empty → fee falls
//!     delta = base_fee × (gas_target - gas_used) / gas_target / 8
//!     next  = base_fee - min(delta, base_fee - FLOOR)
//!
//! Maximum change per block: ±12.5%
//! Floor: MIN_BASE_FEE_WEI (100 Mwei = 0.1 Gwei) — ZBX is cheaper than ETH
//! Starting value: INITIAL_BASE_FEE_WEI (1 Gwei)
//!
//! References:
//!   EIP-1559: https://eips.ethereum.org/EIPS/eip-1559
//!   ZEP-005 (Gas): docs/ZEP-005-dynamic-gas.md

/// Minimum base fee (floor). Never goes below this.
/// Set lower than Ethereum (100 Mwei vs 1 Gwei) — ZBX is cheaper.
pub const MIN_BASE_FEE_WEI: u128 = 100_000_000; // 0.1 Gwei = 100 Mwei

/// Starting base fee at genesis / after a chain restart.
pub const INITIAL_BASE_FEE_WEI: u128 = 1_000_000_000; // 1 Gwei

/// Maximum base fee ZBX allows (sanity cap: 10,000 Gwei).
pub const MAX_BASE_FEE_WEI: u128 = 10_000_000_000_000; // 10,000 Gwei

/// Default gas limit per block (30M gas, same as Ethereum).
pub const BLOCK_GAS_LIMIT: u64 = 30_000_000;

/// Gas target per block = 50% of limit.
pub const BLOCK_GAS_TARGET: u64 = BLOCK_GAS_LIMIT / 2; // 15_000_000

/// Maximum change per block (denominator — 8 means 1/8 = 12.5% max)
const BASE_FEE_CHANGE_DENOMINATOR: u128 = 8;

/// Priority fee (tip) recommended for fast inclusion.
/// Users set this on top of base_fee in EIP-1559 transactions.
pub const RECOMMENDED_PRIORITY_FEE_WEI: u128 = 100_000_000; // 0.1 Gwei

/// Compute the next block's base fee from the parent block's parameters.
///
/// # Arguments
/// * `parent_base_fee`  — parent block's base fee (wei)
/// * `parent_gas_used`  — actual gas consumed in parent block
/// * `parent_gas_limit` — gas limit of parent block (usually BLOCK_GAS_LIMIT)
///
/// # Returns
/// Next block's base fee, clamped to [MIN_BASE_FEE_WEI, MAX_BASE_FEE_WEI].
///
/// # Examples
/// ```
/// use zbx_sequencer::base_fee::{compute_next_base_fee, INITIAL_BASE_FEE_WEI, BLOCK_GAS_LIMIT};
///
/// // Block at exactly 50% → no change
/// let fee = compute_next_base_fee(INITIAL_BASE_FEE_WEI, 15_000_000, BLOCK_GAS_LIMIT);
/// assert_eq!(fee, INITIAL_BASE_FEE_WEI);
///
/// // Block at 100% full → +12.5%
/// let fee = compute_next_base_fee(INITIAL_BASE_FEE_WEI, 30_000_000, BLOCK_GAS_LIMIT);
/// assert_eq!(fee, 1_125_000_000); // 1 Gwei + 12.5%
///
/// // Block empty → -12.5%
/// let fee = compute_next_base_fee(INITIAL_BASE_FEE_WEI, 0, BLOCK_GAS_LIMIT);
/// assert_eq!(fee, 875_000_000); // 1 Gwei - 12.5%
/// ```
pub fn compute_next_base_fee(
    parent_base_fee:  u128,
    parent_gas_used:  u64,
    parent_gas_limit: u64,
) -> u128 {
    let gas_target = (parent_gas_limit / 2) as u128;
    let gas_used   = parent_gas_used as u128;
    let base_fee   = parent_base_fee;

    let next = if gas_used == gas_target {
        // Target hit exactly → fee unchanged
        base_fee

    } else if gas_used > gas_target {
        // Block too full → raise fee
        let gas_delta  = gas_used - gas_target;
        let fee_delta  = base_fee
            .saturating_mul(gas_delta)
            / gas_target
            / BASE_FEE_CHANGE_DENOMINATOR;
        // At least +1 wei (prevents rounding to zero on tiny blocks)
        base_fee.saturating_add(fee_delta.max(1))

    } else {
        // Block too empty → lower fee
        let gas_delta  = gas_target - gas_used;
        let fee_delta  = base_fee
            .saturating_mul(gas_delta)
            / gas_target
            / BASE_FEE_CHANGE_DENOMINATOR;
        // Never go below floor
        base_fee
            .saturating_sub(fee_delta)
            .max(MIN_BASE_FEE_WEI)
    };

    // Hard cap: [MIN, MAX]
    next.clamp(MIN_BASE_FEE_WEI, MAX_BASE_FEE_WEI)
}

/// Convert base fee in wei to Gwei (for display / RPC responses).
///
/// ```
/// use zbx_sequencer::base_fee::wei_to_gwei;
/// assert_eq!(wei_to_gwei(1_500_000_000), 1.5_f64);
/// ```
pub fn wei_to_gwei(wei: u128) -> f64 {
    wei as f64 / 1_000_000_000.0
}

/// Convert Gwei to wei.
pub fn gwei_to_wei(gwei: f64) -> u128 {
    (gwei * 1_000_000_000.0) as u128
}

/// Hex-encode wei value for JSON-RPC responses (0x prefixed, no leading zeros).
///
/// ```
/// use zbx_sequencer::base_fee::wei_to_hex;
/// assert_eq!(wei_to_hex(1_000_000_000), "0x3b9aca00");
/// ```
pub fn wei_to_hex(wei: u128) -> String {
    format!("0x{:x}", wei)
}

/// Recommended max_fee_per_gas for a user transaction.
/// = base_fee × 2 + priority_fee  (gives 1-block buffer for fee rise)
///
/// ```
/// use zbx_sequencer::base_fee::{recommended_max_fee, INITIAL_BASE_FEE_WEI};
/// let max_fee = recommended_max_fee(INITIAL_BASE_FEE_WEI);
/// assert_eq!(max_fee, 2_100_000_000); // 2 Gwei + 0.1 Gwei tip
/// ```
pub fn recommended_max_fee(current_base_fee: u128) -> u128 {
    current_base_fee
        .saturating_mul(2)
        .saturating_add(RECOMMENDED_PRIORITY_FEE_WEI)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Exact-target tests ──────────────────────────────────────────────────

    #[test]
    fn target_hit_no_change() {
        let fee = compute_next_base_fee(INITIAL_BASE_FEE_WEI, BLOCK_GAS_TARGET, BLOCK_GAS_LIMIT);
        assert_eq!(fee, INITIAL_BASE_FEE_WEI, "At 50%% gas: fee must stay the same");
    }

    // ── Full block (+12.5%) ─────────────────────────────────────────────────

    #[test]
    fn full_block_raises_fee_by_12_5_percent() {
        // Block 100% full → +12.5%
        let fee = compute_next_base_fee(INITIAL_BASE_FEE_WEI, BLOCK_GAS_LIMIT, BLOCK_GAS_LIMIT);
        let expected = 1_125_000_000u128; // 1 Gwei + 12.5%
        assert_eq!(fee, expected, "Full block must raise fee by 12.5%%");
    }

    #[test]
    fn seventy_five_percent_full_raises_fee_by_6_25_percent() {
        // Block 75% full → +6.25% (half of max)
        let gas_used = BLOCK_GAS_LIMIT * 3 / 4;
        let fee = compute_next_base_fee(INITIAL_BASE_FEE_WEI, gas_used, BLOCK_GAS_LIMIT);
        let expected = 1_062_500_000u128; // 1 Gwei + 6.25%
        assert_eq!(fee, expected, "75%% full must raise fee by 6.25%%");
    }

    // ── Empty block (-12.5%) ────────────────────────────────────────────────

    #[test]
    fn empty_block_lowers_fee_by_12_5_percent() {
        let fee = compute_next_base_fee(INITIAL_BASE_FEE_WEI, 0, BLOCK_GAS_LIMIT);
        let expected = 875_000_000u128; // 1 Gwei - 12.5%
        assert_eq!(fee, expected, "Empty block must lower fee by 12.5%%");
    }

    #[test]
    fn quarter_full_lowers_fee_by_6_25_percent() {
        let gas_used = BLOCK_GAS_LIMIT / 4;
        let fee = compute_next_base_fee(INITIAL_BASE_FEE_WEI, gas_used, BLOCK_GAS_LIMIT);
        let expected = 937_500_000u128; // 1 Gwei - 6.25%
        assert_eq!(fee, expected, "25%% full must lower fee by 6.25%%");
    }

    // ── Floor clamping ──────────────────────────────────────────────────────

    #[test]
    fn never_goes_below_floor() {
        // Start at floor, keep feeding empty blocks
        let mut fee = MIN_BASE_FEE_WEI;
        for _ in 0..100 {
            fee = compute_next_base_fee(fee, 0, BLOCK_GAS_LIMIT);
            assert!(
                fee >= MIN_BASE_FEE_WEI,
                "Fee ({fee}) went below floor ({MIN_BASE_FEE_WEI})"
            );
        }
        assert_eq!(fee, MIN_BASE_FEE_WEI, "Fee must converge to floor with empty blocks");
    }

    #[test]
    fn floor_is_reached_from_any_starting_point() {
        // Starting from a very low fee
        let fee = compute_next_base_fee(MIN_BASE_FEE_WEI + 1, 0, BLOCK_GAS_LIMIT);
        assert!(fee >= MIN_BASE_FEE_WEI);
    }

    // ── Ceiling clamping ────────────────────────────────────────────────────

    #[test]
    fn never_exceeds_max() {
        let mut fee = MAX_BASE_FEE_WEI - 1;
        for _ in 0..10 {
            fee = compute_next_base_fee(fee, BLOCK_GAS_LIMIT, BLOCK_GAS_LIMIT);
            assert!(
                fee <= MAX_BASE_FEE_WEI,
                "Fee ({fee}) exceeded cap ({MAX_BASE_FEE_WEI})"
            );
        }
    }

    // ── Convergence test ────────────────────────────────────────────────────

    #[test]
    fn fee_converges_at_target_utilization() {
        // If block is always exactly half full, fee must stay stable
        let mut fee = INITIAL_BASE_FEE_WEI;
        for _ in 0..100 {
            let next = compute_next_base_fee(fee, BLOCK_GAS_TARGET, BLOCK_GAS_LIMIT);
            assert_eq!(next, fee, "Fee must be stable at 50%% utilization");
            fee = next;
        }
    }

    #[test]
    fn sustained_full_blocks_approach_max() {
        let mut fee = INITIAL_BASE_FEE_WEI;
        for _ in 0..200 {
            fee = compute_next_base_fee(fee, BLOCK_GAS_LIMIT, BLOCK_GAS_LIMIT);
        }
        assert!(fee > INITIAL_BASE_FEE_WEI * 100, "Fee should have grown significantly");
        assert!(fee <= MAX_BASE_FEE_WEI, "Fee must not exceed cap");
    }

    // ── Utility function tests ──────────────────────────────────────────────

    #[test]
    fn wei_to_gwei_correct() {
        assert_eq!(wei_to_gwei(1_000_000_000), 1.0);
        assert_eq!(wei_to_gwei(1_500_000_000), 1.5);
        assert_eq!(wei_to_gwei(0), 0.0);
    }

    #[test]
    fn wei_to_hex_correct() {
        assert_eq!(wei_to_hex(1_000_000_000), "0x3b9aca00");
        assert_eq!(wei_to_hex(0), "0x0");
    }

    #[test]
    fn recommended_max_fee_gives_buffer() {
        // max_fee = base_fee * 2 + priority_fee
        let max = recommended_max_fee(INITIAL_BASE_FEE_WEI);
        assert_eq!(max, 2_100_000_000u128); // 2 Gwei + 0.1 Gwei
        // Should always be > base_fee so tx doesn't get stuck
        assert!(max > INITIAL_BASE_FEE_WEI);
    }

    // ── ZBX-specific: cheap floor ───────────────────────────────────────────

    #[test]
    fn zbx_floor_is_lower_than_ethereum() {
        // Ethereum floor = 1 Gwei, ZBX floor = 0.1 Gwei
        const ETH_MIN: u128 = 1_000_000_000;
        assert!(
            MIN_BASE_FEE_WEI < ETH_MIN,
            "ZBX floor must be cheaper than Ethereum"
        );
    }

    // ── Symmetry: same delta up and down at same distance from target ───────

    #[test]
    fn symmetric_adjustment_at_equal_distance() {
        // 25% above target (75% utilization) should increase by same amount
        // as 25% below target (25% utilization) would decrease
        let gas_high = BLOCK_GAS_TARGET + BLOCK_GAS_TARGET / 2; // 75%
        let gas_low  = BLOCK_GAS_TARGET / 2;                     // 25%

        let fee_high = compute_next_base_fee(INITIAL_BASE_FEE_WEI, gas_high, BLOCK_GAS_LIMIT);
        let fee_low  = compute_next_base_fee(INITIAL_BASE_FEE_WEI, gas_low, BLOCK_GAS_LIMIT);

        let increase = fee_high - INITIAL_BASE_FEE_WEI;
        let decrease = INITIAL_BASE_FEE_WEI - fee_low;

        assert_eq!(increase, decrease, "Symmetric utilization must give symmetric fee change");
    }
}