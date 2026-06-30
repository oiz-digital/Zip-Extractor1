//! LP token accounting — tracks LP balances per pair per account.

use std::collections::HashMap;
use zbx_types::address::Address;
use crate::pair::PairId;

// ── Error ─────────────────────────────────────────────────────────────────────

/// Typed error for LP token operations.
///
/// Previously `burn` returned `Result<(), &'static str>` — an untyped string
/// error that makes it impossible for callers to match on specific failure modes.
/// This enum replaces that with a proper error type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LpError {
    /// The account holds fewer LP tokens than the requested burn amount.
    InsufficientBalance { have: u128, need: u128 },
    /// Transfer to the zero address is not allowed.
    ZeroAddressTransfer,
}

impl std::fmt::Display for LpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InsufficientBalance { have, need } =>
                write!(f, "insufficient LP balance: have {have}, need {need}"),
            Self::ZeroAddressTransfer =>
                write!(f, "LP transfer to the zero address is not allowed"),
        }
    }
}

impl std::error::Error for LpError {}

// ── LpRegistry ────────────────────────────────────────────────────────────────

/// LP token balances for all pairs.
#[derive(Debug, Default)]
pub struct LpRegistry {
    /// pair_id → address → balance
    balances: HashMap<PairId, HashMap<Address, u128>>,
    total_supply: HashMap<PairId, u128>,
}

impl LpRegistry {
    pub fn new() -> Self { Self::default() }

    pub fn balance(&self, pair: &PairId, owner: &Address) -> u128 {
        self.balances.get(pair)
            .and_then(|m| m.get(owner))
            .copied()
            .unwrap_or(0)
    }

    pub fn total_supply(&self, pair: &PairId) -> u128 {
        self.total_supply.get(pair).copied().unwrap_or(0)
    }

    /// Mint `amount` LP tokens for `pair` to `to`.
    ///
    /// Silently ignores zero-amount mints (no-op).
    pub fn mint(&mut self, pair: &PairId, to: Address, amount: u128) {
        if amount == 0 { return; }
        *self.balances.entry(pair.clone()).or_default().entry(to).or_insert(0) += amount;
        *self.total_supply.entry(pair.clone()).or_insert(0) += amount;
    }

    /// Burn `amount` LP tokens for `pair` from `from`.
    ///
    /// Returns `Err(LpError::InsufficientBalance)` if the account holds fewer
    /// than `amount` tokens.
    pub fn burn(&mut self, pair: &PairId, from: Address, amount: u128) -> Result<(), LpError> {
        let bal = self.balances.entry(pair.clone()).or_default().entry(from).or_insert(0);
        if *bal < amount {
            return Err(LpError::InsufficientBalance { have: *bal, need: amount });
        }
        *bal -= amount;
        *self.total_supply.entry(pair.clone()).or_insert(0) =
            self.total_supply[pair].saturating_sub(amount);
        Ok(())
    }

    /// Transfer `amount` LP tokens for `pair` from `from` to `to`.
    ///
    /// Returns `Err(LpError::ZeroAddressTransfer)` if `to` is the zero address.
    /// Returns `Err(LpError::InsufficientBalance)` if `from` holds fewer than
    /// `amount` tokens.
    pub fn transfer(
        &mut self,
        pair:   &PairId,
        from:   Address,
        to:     Address,
        amount: u128,
    ) -> Result<(), LpError> {
        if to == Address([0u8; 20]) {
            return Err(LpError::ZeroAddressTransfer);
        }
        self.burn(pair, from, amount)?;
        self.mint(pair, to, amount);
        Ok(())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(n: u8) -> Address { Address([n; 20]) }
    fn pair_id() -> PairId { PairId::new(addr(1), addr(2)) }

    #[test]
    fn mint_and_balance() {
        let mut reg = LpRegistry::new();
        reg.mint(&pair_id(), addr(10), 1_000);
        assert_eq!(reg.balance(&pair_id(), &addr(10)), 1_000);
        assert_eq!(reg.total_supply(&pair_id()), 1_000);
    }

    #[test]
    fn burn_reduces_balance() {
        let mut reg = LpRegistry::new();
        reg.mint(&pair_id(), addr(10), 1_000);
        reg.burn(&pair_id(), addr(10), 400).unwrap();
        assert_eq!(reg.balance(&pair_id(), &addr(10)), 600);
        assert_eq!(reg.total_supply(&pair_id()), 600);
    }

    #[test]
    fn burn_insufficient_returns_typed_error() {
        let mut reg = LpRegistry::new();
        reg.mint(&pair_id(), addr(10), 100);
        let err = reg.burn(&pair_id(), addr(10), 200);
        assert!(matches!(err, Err(LpError::InsufficientBalance { have: 100, need: 200 })));
    }

    #[test]
    fn transfer_moves_tokens() {
        let mut reg = LpRegistry::new();
        reg.mint(&pair_id(), addr(10), 1_000);
        reg.transfer(&pair_id(), addr(10), addr(20), 300).unwrap();
        assert_eq!(reg.balance(&pair_id(), &addr(10)), 700);
        assert_eq!(reg.balance(&pair_id(), &addr(20)), 300);
        assert_eq!(reg.total_supply(&pair_id()), 1_000); // unchanged
    }

    #[test]
    fn transfer_to_zero_address_rejected() {
        let mut reg = LpRegistry::new();
        reg.mint(&pair_id(), addr(10), 1_000);
        let err = reg.transfer(&pair_id(), addr(10), addr(0), 100);
        assert!(matches!(err, Err(LpError::ZeroAddressTransfer)));
        // Balance unchanged
        assert_eq!(reg.balance(&pair_id(), &addr(10)), 1_000);
    }

    #[test]
    fn mint_zero_is_noop() {
        let mut reg = LpRegistry::new();
        reg.mint(&pair_id(), addr(10), 0);
        assert_eq!(reg.total_supply(&pair_id()), 0);
    }
}
