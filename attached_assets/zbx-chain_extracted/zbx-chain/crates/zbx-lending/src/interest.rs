//! Interest rate model — jump-rate model used by the lending protocol.

/// Interest rate model parameters (all rates per-second, scaled by 1e18).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct JumpRateModel {
    /// Base borrow rate at 0% utilisation (per-second).
    pub base_rate_per_sec: u128,
    /// Rate multiplier below the kink (per-second per unit utilisation).
    pub multiplier_per_sec: u128,
    /// Additional rate multiplier above the kink.
    pub jump_multiplier_per_sec: u128,
    /// Utilisation at which the jump applies (scaled 1e18).
    pub kink: u128,
}

impl JumpRateModel {
    /// Conservative default: 2% base, 20% at kink (80%), 100% at 100% util.
    pub fn default_conservative() -> Self {
        Self {
            base_rate_per_sec:       634_195_839,        // ~2%/yr
            multiplier_per_sec:      19_025_875_190,     // ~60%/yr at kink
            jump_multiplier_per_sec: 253_678_335_870,    // ~800%/yr above kink
            kink:                    800_000_000_000_000_000, // 80%
        }
    }

    /// Utilisation rate = borrows / (cash + borrows - reserves).
    pub fn utilisation(cash: u128, borrows: u128, reserves: u128) -> u128 {
        let denominator = cash + borrows.saturating_sub(reserves);
        if denominator == 0 { return 0; }
        borrows * 1_000_000_000_000_000_000 / denominator
    }

    /// Borrow rate per second at the given utilisation (1e18 scale).
    pub fn borrow_rate(&self, util: u128) -> u128 {
        if util <= self.kink {
            self.base_rate_per_sec
                + self.multiplier_per_sec * util / 1_000_000_000_000_000_000
        } else {
            let normal = self.base_rate_per_sec
                + self.multiplier_per_sec * self.kink / 1_000_000_000_000_000_000;
            let excess = util - self.kink;
            normal + self.jump_multiplier_per_sec * excess / 1_000_000_000_000_000_000
        }
    }

    /// Supply rate per second at the given utilisation, after reserving `reserve_factor_bps`.
    pub fn supply_rate(&self, util: u128, reserve_factor_bps: u32) -> u128 {
        let borrow = self.borrow_rate(util);
        let reserve_fraction = (reserve_factor_bps as u128) * 1_000_000_000_000_000_000 / 10_000;
        borrow * util / 1_000_000_000_000_000_000
            * (1_000_000_000_000_000_000 - reserve_fraction)
            / 1_000_000_000_000_000_000
    }
}
