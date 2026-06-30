//! EIP-2981 compatible royalty registry for ZEP-721/ZEP-1155 collections.

use std::collections::HashMap;
use zbx_types::address::Address;
use crate::mint::TokenId;

/// Basis points denominator (100% = 10_000 bps).
pub const BPS_DENOM: u64 = 10_000;

/// Per-token or per-collection royalty configuration.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RoyaltyInfo {
    /// Recipient of royalty payments.
    pub receiver: Address,
    /// Royalty rate in basis points (e.g. 250 = 2.5%).
    pub bps:      u64,
}

/// Royalty registry — supports collection-level and per-token overrides.
#[derive(Debug, Default)]
pub struct RoyaltyRegistry {
    /// Collection-level default royalty.
    default_royalty: Option<RoyaltyInfo>,
    /// Per-token royalty overrides.
    token_royalties: HashMap<TokenId, RoyaltyInfo>,
}

impl RoyaltyRegistry {
    pub fn new() -> Self { Self::default() }

    pub fn set_default_royalty(&mut self, receiver: Address, bps: u64) -> Result<(), &'static str> {
        if bps > BPS_DENOM { return Err("royalty exceeds 100%"); }
        self.default_royalty = Some(RoyaltyInfo { receiver, bps });
        Ok(())
    }

    pub fn set_token_royalty(&mut self, token_id: TokenId, receiver: Address, bps: u64) -> Result<(), &'static str> {
        if bps > BPS_DENOM { return Err("royalty exceeds 100%"); }
        self.token_royalties.insert(token_id, RoyaltyInfo { receiver, bps });
        Ok(())
    }

    /// Returns `(receiver, royalty_amount)` for a sale of `sale_price`.
    pub fn royalty_info(&self, token_id: TokenId, sale_price: u128) -> Option<(Address, u128)> {
        let info = self.token_royalties.get(&token_id)
            .or(self.default_royalty.as_ref())?;
        let amount = sale_price * info.bps as u128 / BPS_DENOM as u128;
        Some((info.receiver, amount))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(b: u8) -> Address { [b; 20] }

    #[test]
    fn default_royalty_applied_when_no_token_override() {
        let mut reg = RoyaltyRegistry::new();
        reg.set_default_royalty(addr(1), 250).unwrap(); // 2.5%
        let (recv, amt) = reg.royalty_info(0, 10_000).unwrap();
        assert_eq!(recv, addr(1));
        assert_eq!(amt, 250);
    }

    #[test]
    fn token_royalty_overrides_default() {
        let mut reg = RoyaltyRegistry::new();
        reg.set_default_royalty(addr(1), 250).unwrap();
        reg.set_token_royalty(5, addr(2), 500).unwrap(); // 5% override on token 5
        let (recv, amt) = reg.royalty_info(5, 10_000).unwrap();
        assert_eq!(recv, addr(2));
        assert_eq!(amt, 500);
    }

    #[test]
    fn no_royalty_when_none_registered() {
        let reg = RoyaltyRegistry::new();
        assert!(reg.royalty_info(0, 10_000).is_none());
    }

    #[test]
    fn royalty_exceeds_100pct_rejected() {
        let mut reg = RoyaltyRegistry::new();
        assert!(reg.set_default_royalty(addr(1), BPS_DENOM + 1).is_err());
        assert!(reg.set_token_royalty(0, addr(1), BPS_DENOM + 1).is_err());
    }

    #[test]
    fn zero_royalty_accepted() {
        let mut reg = RoyaltyRegistry::new();
        reg.set_default_royalty(addr(1), 0).unwrap();
        let (_, amt) = reg.royalty_info(0, 10_000).unwrap();
        assert_eq!(amt, 0);
    }

    #[test]
    fn royalty_rounds_down() {
        let mut reg = RoyaltyRegistry::new();
        reg.set_default_royalty(addr(1), 333).unwrap(); // 3.33%
        let (_, amt) = reg.royalty_info(0, 1000).unwrap();
        assert_eq!(amt, 33); // floor(1000 * 333 / 10000)
    }
}
