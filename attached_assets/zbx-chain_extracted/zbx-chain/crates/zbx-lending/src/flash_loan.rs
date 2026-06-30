//! Flash loan provider — EIP-3156 style single-transaction flash loans.
//!
//! Fee: 0.09% (9 bps) of the borrowed amount, collected as protocol revenue.
//! Reentrancy guard: a boolean lock prevents nested flash loans on the same market.
//! Max flash loan: 50% of the market's available liquidity per call.
//!
//! ## Security fix FLASH-03
//! `collect_fees()` now requires `caller == admin`.  Without this guard any
//! external caller could drain the entire `fee_reserve` to themselves.

use std::collections::HashMap;
use zbx_types::address::Address;
use crate::market::MarketId;

/// Flash loan fee in basis points (9 bps = 0.09%).
pub const FLASH_LOAN_FEE_BPS: u128 = 9;
/// Maximum flash loan as a fraction of available liquidity (50%, scaled 1e18).
pub const MAX_FLASH_LOAN_FRACTION: u128 = 500_000_000_000_000_000; // 50%

/// Flash loan errors.
#[derive(Debug, thiserror::Error)]
pub enum FlashLoanError {
    #[error("market {0} not found")]
    MarketNotFound(String),
    #[error("flash loan reentrancy detected on market {0}")]
    Reentrancy(String),
    #[error("requested amount {requested} exceeds maximum {max} (50% of liquidity)")]
    ExceedsMaximum { requested: u128, max: u128 },
    #[error("borrower did not repay: expected {expected}, returned {returned}")]
    RepaymentInsufficient { expected: u128, returned: u128 },
    #[error("zero amount flash loan not allowed")]
    ZeroAmount,
    #[error("flash loan fee overflow — amount too large")]
    FeeOverflow,
    /// FLASH-03: unauthorised fee withdrawal attempt.
    #[error("caller is not admin — cannot collect fees")]
    NotAdmin,
}

/// Represents the callback interface a borrower must implement.
/// In practice this would be a trait with async dispatch; here we model
/// the callback as a closure return value for testability.
#[derive(Debug, Clone)]
pub struct FlashLoanReceipt {
    pub borrower: Address,
    pub market:   MarketId,
    pub amount:   u128,
    pub fee:      u128,
    /// Amount the borrower actually returned (must equal amount + fee).
    pub returned: u128,
}

/// Per-market flash loan liquidity pool state.
#[derive(Debug, Clone)]
pub struct FlashMarket {
    pub market:     MarketId,
    /// Available liquidity that can be loaned out.
    pub liquidity:  u128,
    /// Accumulated protocol fees collected.
    pub fee_reserve: u128,
    /// Reentrancy lock.
    locked: bool,
}

impl FlashMarket {
    pub fn new(market: MarketId, liquidity: u128) -> Self {
        Self { market, liquidity, fee_reserve: 0, locked: false }
    }
}

/// FlashLoanProvider — manages flash loan pools across multiple markets.
#[derive(Debug)]
pub struct FlashLoanProvider {
    markets: HashMap<MarketId, FlashMarket>,
    /// FLASH-03: only this address may call `collect_fees`.
    pub admin: Address,
}

impl FlashLoanProvider {
    pub fn new(admin: Address) -> Self {
        Self { markets: HashMap::new(), admin }
    }

    pub fn add_market(&mut self, market: FlashMarket) {
        self.markets.insert(market.market.clone(), market);
    }

    /// Maximum flash loan amount for the given market (50% of liquidity).
    pub fn max_flash_loan(&self, market: &MarketId) -> u128 {
        self.markets.get(market).map(|m| {
            m.liquidity * MAX_FLASH_LOAN_FRACTION / 1_000_000_000_000_000_000
        }).unwrap_or(0)
    }

    /// Compute the flash loan fee for `amount` (0.09%).
    pub fn flash_fee(amount: u128) -> Result<u128, FlashLoanError> {
        amount.checked_mul(FLASH_LOAN_FEE_BPS)
            .and_then(|v| v.checked_div(10_000))
            .ok_or(FlashLoanError::FeeOverflow)
    }

    /// Initiate a flash loan.
    ///
    /// `returned` simulates what the borrower returns after the callback.
    /// In production, returned is determined by the on-chain callback execution.
    pub fn flash_loan(
        &mut self,
        borrower: Address,
        market_id: &MarketId,
        amount: u128,
        returned: u128, // borrower's callback return (simulated in tests)
    ) -> Result<FlashLoanReceipt, FlashLoanError> {
        if amount == 0 {
            return Err(FlashLoanError::ZeroAmount);
        }

        let market = self.markets.get_mut(market_id)
            .ok_or_else(|| FlashLoanError::MarketNotFound(market_id.0.clone()))?;

        if market.locked {
            return Err(FlashLoanError::Reentrancy(market_id.0.clone()));
        }

        let max = market.liquidity * MAX_FLASH_LOAN_FRACTION / 1_000_000_000_000_000_000;
        if amount > max {
            return Err(FlashLoanError::ExceedsMaximum { requested: amount, max });
        }

        let fee = Self::flash_fee(amount)?;
        let expected_return = amount.saturating_add(fee);

        // Lock — reentrancy guard.
        market.locked = true;

        // ── Callback executed here (borrower uses the funds) ──────────────────
        // In production: EVM call into borrower.onFlashLoan(initiator, token, amount, fee, data)
        // For simulation: `returned` is passed by the caller.

        // ── Check repayment ───────────────────────────────────────────────────
        if returned < expected_return {
            market.locked = false;
            return Err(FlashLoanError::RepaymentInsufficient {
                expected: expected_return,
                returned,
            });
        }

        // Collect fee into reserve, credit excess back to liquidity.
        market.liquidity = market.liquidity.saturating_add(fee);
        market.fee_reserve = market.fee_reserve.saturating_add(fee);
        market.locked = false;

        Ok(FlashLoanReceipt {
            borrower,
            market: market_id.clone(),
            amount,
            fee,
            returned,
        })
    }

    pub fn market(&self, id: &MarketId) -> Option<&FlashMarket> {
        self.markets.get(id)
    }

    /// Withdraw accumulated protocol fees to the caller.
    ///
    /// ## FLASH-03 fix
    /// Only the `admin` address recorded at construction time may drain fees.
    /// Previously this function had no access control; any caller could zero-out
    /// `fee_reserve` and redirect accumulated protocol revenue to themselves.
    pub fn collect_fees(
        &mut self,
        caller:    Address,
        market_id: &MarketId,
    ) -> Result<u128, FlashLoanError> {
        if caller != self.admin {
            return Err(FlashLoanError::NotAdmin);
        }
        Ok(self.markets.get_mut(market_id).map(|m| {
            let fees = m.fee_reserve;
            m.fee_reserve = 0;
            fees
        }).unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mid(s: &str) -> MarketId { MarketId(s.to_string()) }
    fn addr(v: u8) -> Address { Address([v; 20]) }

    const ADMIN: u8 = 99;

    fn provider_with_market(liq: u128) -> (FlashLoanProvider, MarketId) {
        let mut fp = FlashLoanProvider::new(addr(ADMIN));
        let m = mid("ZBX");
        fp.add_market(FlashMarket::new(m.clone(), liq));
        (fp, m)
    }

    #[test]
    fn successful_flash_loan() {
        let (mut fp, m) = provider_with_market(1_000_000);
        let max = fp.max_flash_loan(&m);
        let fee = FlashLoanProvider::flash_fee(max).unwrap();
        let receipt = fp.flash_loan(addr(1), &m, max, max + fee).unwrap();
        assert_eq!(receipt.amount, max);
        assert_eq!(receipt.fee, fee);
        assert_eq!(fee, max * 9 / 10_000);
    }

    #[test]
    fn fee_collected_into_reserve() {
        let (mut fp, m) = provider_with_market(2_000_000);
        let amount = 100_000u128;
        let fee = FlashLoanProvider::flash_fee(amount).unwrap();
        fp.flash_loan(addr(1), &m, amount, amount + fee).unwrap();
        // Admin can collect.
        assert_eq!(fp.collect_fees(addr(ADMIN), &m).unwrap(), fee);
        // Second call: already zeroed.
        assert_eq!(fp.collect_fees(addr(ADMIN), &m).unwrap(), 0);
    }

    /// FLASH-03: non-admin must be rejected.
    #[test]
    fn collect_fees_non_admin_rejected() {
        let (mut fp, m) = provider_with_market(2_000_000);
        let amount = 100_000u128;
        let fee = FlashLoanProvider::flash_fee(amount).unwrap();
        fp.flash_loan(addr(1), &m, amount, amount + fee).unwrap();
        let err = fp.collect_fees(addr(1), &m).unwrap_err();
        assert!(matches!(err, FlashLoanError::NotAdmin));
        // Fee reserve intact after failed withdrawal.
        assert_eq!(fp.collect_fees(addr(ADMIN), &m).unwrap(), fee);
    }

    #[test]
    fn exceeds_maximum_rejected() {
        let (mut fp, m) = provider_with_market(1_000_000);
        let max = fp.max_flash_loan(&m);
        let err = fp.flash_loan(addr(1), &m, max + 1, max + 1).unwrap_err();
        assert!(matches!(err, FlashLoanError::ExceedsMaximum { .. }));
    }

    #[test]
    fn underpayment_rejected() {
        let (mut fp, m) = provider_with_market(1_000_000);
        let amount = 50_000u128;
        let err = fp.flash_loan(addr(1), &m, amount, amount).unwrap_err();
        assert!(matches!(err, FlashLoanError::RepaymentInsufficient { .. }));
    }

    #[test]
    fn zero_amount_rejected() {
        let (mut fp, m) = provider_with_market(1_000_000);
        assert!(matches!(fp.flash_loan(addr(1), &m, 0, 0), Err(FlashLoanError::ZeroAmount)));
    }

    #[test]
    fn unknown_market_rejected() {
        let mut fp = FlashLoanProvider::new(addr(ADMIN));
        let err = fp.flash_loan(addr(1), &mid("UNKNOWN"), 100, 100).unwrap_err();
        assert!(matches!(err, FlashLoanError::MarketNotFound(_)));
    }

    #[test]
    fn reentrancy_guard_fires() {
        let mut fp = FlashLoanProvider::new(addr(ADMIN));
        let m = mid("ZBX");
        fp.add_market(FlashMarket { market: m.clone(), liquidity: 1_000_000, fee_reserve: 0, locked: true });
        let err = fp.flash_loan(addr(1), &m, 1, 1).unwrap_err();
        assert!(matches!(err, FlashLoanError::Reentrancy(_)));
    }
}
