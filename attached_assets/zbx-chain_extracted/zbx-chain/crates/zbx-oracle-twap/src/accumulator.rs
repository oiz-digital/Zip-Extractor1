//! Cumulative price accumulator for TWAP calculation.

use serde::{Serialize, Deserialize};

/// A snapshot of the cumulative price at one point in time.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct PriceAccumulator {
    /// Cumulative price × time (token0/token1 * seconds)
    /// Uses u256-equivalent (we store as u128 for simplicity)
    pub price0_cumulative: u128,
    pub price1_cumulative: u128,
    /// Block timestamp when this snapshot was taken
    pub block_timestamp:   u64,
}

impl PriceAccumulator {
    /// Update the accumulator with the current spot price and elapsed time.
    ///
    /// Called once per block in the AMM contract.
    pub fn update(&mut self, spot_price: u128, now: u64) {
        let elapsed = now.saturating_sub(self.block_timestamp);
        if elapsed == 0 { return; }
        // price0_cumulative += spot_price × elapsed
        self.price0_cumulative = self.price0_cumulative.wrapping_add(
            spot_price.saturating_mul(elapsed as u128)
        );
        // price1_cumulative is the inverse (1/price × elapsed)
        if spot_price > 0 {
            let inv_price = u128::MAX / spot_price;
            self.price1_cumulative = self.price1_cumulative.wrapping_add(
                inv_price.saturating_mul(elapsed as u128)
            );
        }
        self.block_timestamp = now;
    }
}

/// A TWAP observation window (two snapshots at different times).
#[derive(Clone, Debug)]
pub struct TwapWindow {
    pub start: PriceAccumulator,
    pub end:   PriceAccumulator,
}

impl TwapWindow {
    /// Calculate TWAP from two accumulator snapshots.
    ///
    /// TWAP = (cumulative_end - cumulative_start) / (time_end - time_start)
    pub fn twap_price(&self) -> Option<u128> {
        let dt = self.end.block_timestamp.saturating_sub(self.start.block_timestamp);
        if dt == 0 { return None; }
        let delta_cumulative = self.end.price0_cumulative
            .wrapping_sub(self.start.price0_cumulative);
        Some(delta_cumulative / dt as u128)
    }

    /// Window duration in seconds.
    pub fn duration_secs(&self) -> u64 {
        self.end.block_timestamp.saturating_sub(self.start.block_timestamp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twap_averages_correctly() {
        let mut acc = PriceAccumulator {
            price0_cumulative: 0,
            price1_cumulative: 0,
            block_timestamp:   1000,
        };
        let start = acc;

        // Price = 250 (meaning $2.50 in our scale) for 60 seconds
        acc.update(250, 1060);
        // Price spikes to 9999 for 1 second (flash attack)
        acc.update(9999, 1061);
        // Back to 251 for 60 seconds
        acc.update(251, 1121);

        let window = TwapWindow { start, end: acc };
        let twap = window.twap_price().unwrap();
        // Spike only affected 1/121 seconds → TWAP barely moved
        assert!(twap > 200 && twap < 400); // still near $2.50
        assert!(twap < 1000); // NOT near the spike
    }

    #[test]
    fn twap_zero_duration() {
        let acc = PriceAccumulator { price0_cumulative: 100, price1_cumulative: 0, block_timestamp: 1000 };
        let window = TwapWindow { start: acc, end: acc };
        assert!(window.twap_price().is_none());
    }
}