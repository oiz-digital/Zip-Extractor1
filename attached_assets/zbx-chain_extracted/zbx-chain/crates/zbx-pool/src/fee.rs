//! Protocol fee configuration (ZEP-005).

/// Fee tiers in basis points.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FeeTier {
    /// 0.05% — stablecoin pairs
    Lowest  = 5,
    /// 0.30% — standard pairs (default)
    Standard = 30,
    /// 1.00% — exotic/volatile pairs
    High    = 100,
}

impl FeeTier {
    /// Fee in basis points.
    pub fn bps(self) -> u32 { self as u32 }

    /// Protocol's share of the swap fee (20% of total fee, in basis points).
    pub fn protocol_bps(self) -> u32 { self.bps() / 5 }

    /// LP share of the swap fee (80% of total fee, in basis points).
    pub fn lp_bps(self) -> u32 { self.bps() - self.protocol_bps() }
}

impl Default for FeeTier {
    fn default() -> Self { Self::Standard }
}

/// Apply the protocol fee to an amount (returns (lp_fee, protocol_fee)).
pub fn split_fee(amount_in: u128, tier: FeeTier) -> (u128, u128) {
    let total_fee = amount_in * (tier.bps() as u128) / 10_000;
    let protocol  = amount_in * (tier.protocol_bps() as u128) / 10_000;
    let lp        = total_fee.saturating_sub(protocol);
    (lp, protocol)
}
