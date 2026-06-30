//! MEV redistribution — captured MEV goes to stakers and community fund.
//!
//! ZBX Chain MEV redistribution policy:
//!   - 30% to active stakers (proportional to stake)
//!   - 50% to community governance treasury
//!   - 20% burned (deflationary pressure)
//!
//! This aligns validator incentives: validators earn MEV only through
//! the redistribution mechanism, not by frontrunning users directly.

/// MEV redistribution policy.
pub struct MevRedistribution {
    /// Fraction (in basis points) going to stakers. Default: 3000 (30%).
    pub staker_bps: u16,
    /// Fraction going to treasury. Default: 5000 (50%).
    pub treasury_bps: u16,
    /// Fraction burned. Default: 2000 (20%).
    pub burn_bps: u16,
}

impl Default for MevRedistribution {
    fn default() -> Self {
        Self { staker_bps: 3000, treasury_bps: 5000, burn_bps: 2000 }
    }
}

/// Result of distributing a MEV capture.
#[derive(Debug, Clone)]
pub struct DistributionResult {
    pub total_mev:  u128,
    pub to_stakers: u128,
    pub to_treasury: u128,
    pub burned:     u128,
}

impl MevRedistribution {
    pub fn new(staker_bps: u16, treasury_bps: u16, burn_bps: u16) -> Self {
        assert_eq!(staker_bps as u32 + treasury_bps as u32 + burn_bps as u32, 10000,
            "distribution must sum to 100%");
        Self { staker_bps, treasury_bps, burn_bps }
    }

    /// Compute distribution amounts for a captured MEV amount.
    pub fn distribute(&self, total_mev: u128) -> DistributionResult {
        let to_stakers  = total_mev * self.staker_bps as u128 / 10_000;
        let burned      = total_mev * self.burn_bps    as u128 / 10_000;
        let to_treasury = total_mev - to_stakers - burned;
        DistributionResult { total_mev, to_stakers, to_treasury, burned }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn distribution_sums_to_total() {
        let r = MevRedistribution::default();
        let d = r.distribute(1_000_000_000_000_000_000); // 1 ZBX
        assert_eq!(d.to_stakers + d.to_treasury + d.burned, d.total_mev);
    }
    #[test]
    fn percentages_correct() {
        let r = MevRedistribution::default();
        let d = r.distribute(10_000);
        assert_eq!(d.to_stakers,  3_000, "30% to stakers");
        assert_eq!(d.to_treasury, 5_000, "50% to treasury");
        assert_eq!(d.burned,      2_000, "20% burned");
    }
}