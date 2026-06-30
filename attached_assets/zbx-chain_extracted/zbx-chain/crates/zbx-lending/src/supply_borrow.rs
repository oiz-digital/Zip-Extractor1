//! SupplyBorrowEngine — full supply/borrow/repay/redeem engine with:
//!   * Compound-style borrow index for per-account interest accrual
//!   * Health factor check before any borrow (HF must be > 1.0 after borrow)
//!   * Per-market borrow caps (governance-set)
//!   * Close factor 50%: liquidators may repay at most 50% of a position per call
//!   * cToken (receipt token) tracking for supply positions

use std::collections::HashMap;
use zbx_types::address::Address;
use crate::market::{MarketId, Market, AccountMarket, MarketRegistry};
use crate::collateral::CollateralRegistry;
use crate::interest::JumpRateModel;
use crate::liquidation::{LiquidationParams, LiquidationError, compute_seized_collateral};

/// Close factor: max fraction of debt a liquidator may repay in one call (50%, scaled 1e18).
pub const CLOSE_FACTOR: u128 = 500_000_000_000_000_000;
/// Minimum health factor required after a borrow (1.05 × 1e18 = 5% buffer).
pub const MIN_HEALTH_FACTOR: u128 = 1_050_000_000_000_000_000;
/// Scale factor (1e18).
const SCALE: u128 = 1_000_000_000_000_000_000;

/// Supply/borrow engine errors.
#[derive(Debug, thiserror::Error)]
pub enum SbError {
    #[error("market {0} not found")]
    MarketNotFound(String),
    #[error("zero amount not allowed")]
    ZeroAmount,
    #[error("insufficient cToken balance: have {have}, need {need}")]
    InsufficientCTokens { have: u128, need: u128 },
    #[error("borrow would breach borrow cap: cap {cap}, current {current}, requested {amount}")]
    BorrowCapExceeded { cap: u128, current: u128, amount: u128 },
    #[error("borrow would reduce health factor below minimum (hf_after={hf_after})")]
    HealthFactorTooLow { hf_after: u128 },
    #[error("repay amount {repay} exceeds close factor limit {limit}")]
    ExceedsCloseFactor { repay: u128, limit: u128 },
    #[error("position is healthy — cannot liquidate")]
    NotLiquidatable,
    #[error("insufficient repay balance")]
    InsufficientBalance,
    #[error("liquidation error: {0}")]
    Liquidation(#[from] LiquidationError),
}

/// Per-market borrow cap and interest model.
#[derive(Debug, Clone)]
pub struct MarketConfig {
    pub market:       MarketId,
    /// Maximum total borrows allowed (0 = uncapped).
    pub borrow_cap:   u128,
    pub interest:     JumpRateModel,
    /// Current global borrow index (1e18 = no accrual yet).
    pub borrow_index: u128,
    /// Last accrual block.
    pub last_accrual_block: u64,
}

impl MarketConfig {
    pub fn new(market: MarketId) -> Self {
        Self {
            market,
            borrow_cap:         0,
            interest:           JumpRateModel::default_conservative(),
            borrow_index:       SCALE,
            last_accrual_block: 0,
        }
    }

    /// Accrue interest: update borrow_index for elapsed blocks.
    /// `blocks_elapsed` = current_block - last_accrual_block.
    pub fn accrue(&mut self, market: &Market, blocks_elapsed: u64) {
        if blocks_elapsed == 0 { return; }
        let util = JumpRateModel::utilisation(
            market.total_supply.saturating_sub(market.total_borrows),
            market.total_borrows,
            market.reserves,
        );
        // borrow_rate per block (approx 6s block → /yr × 6/31_536_000)
        let borrow_rate_per_sec = self.interest.borrow_rate(util);
        let borrow_rate_per_block = borrow_rate_per_sec * 6; // 6 seconds per block
        let interest_factor = borrow_rate_per_block
            .saturating_mul(blocks_elapsed as u128)
            / SCALE;
        self.borrow_index = self.borrow_index
            .saturating_add(self.borrow_index * interest_factor / SCALE);
        self.last_accrual_block += blocks_elapsed;
    }
}

/// Account borrow state per market.
#[derive(Debug, Clone, Default)]
pub struct BorrowSnapshot {
    /// Principal borrow balance at `index_snapshot`.
    pub principal: u128,
    /// Borrow index at time of last interaction.
    pub index_snapshot: u128,
}

impl BorrowSnapshot {
    /// Current borrow balance accounting for interest accrual.
    pub fn current_balance(&self, current_index: u128) -> u128 {
        if self.principal == 0 || self.index_snapshot == 0 { return self.principal; }
        self.principal * current_index / self.index_snapshot
    }
}

/// Full supply/borrow engine.
#[derive(Debug, Default)]
pub struct SupplyBorrowEngine {
    configs:      HashMap<MarketId, MarketConfig>,
    registries:   MarketRegistry,
    collateral:   CollateralRegistry,
    /// cToken balances per (account, market).
    c_tokens:     HashMap<(Address, MarketId), u128>,
    /// Borrow snapshots per (account, market).
    borrows:      HashMap<(Address, MarketId), BorrowSnapshot>,
    /// ZBX-denominated prices per market (1e18 scale).
    prices:       HashMap<MarketId, u128>,
}

impl SupplyBorrowEngine {
    pub fn new() -> Self { Self::default() }

    pub fn add_market(&mut self, market: Market, config: MarketConfig) {
        self.registries.add_market(market);
        self.configs.insert(config.market.clone(), config);
    }

    pub fn set_price(&mut self, market: MarketId, price: u128) {
        self.prices.insert(market, price);
    }

    pub fn set_collateral_factor(
        &mut self,
        cf: crate::collateral::CollateralFactor,
    ) {
        self.collateral.set_factor(cf);
    }

    // ── Internal helpers ───────────────────────────────────────────────────

    fn accrue(&mut self, market_id: &MarketId, current_block: u64) {
        if let (Some(cfg), Some(mkt)) = (
            self.configs.get_mut(market_id),
            self.registries.market(market_id),
        ) {
            let elapsed = current_block.saturating_sub(cfg.last_accrual_block);
            cfg.accrue(mkt, elapsed);
        }
    }

    fn current_borrow_balance(&self, addr: &Address, market_id: &MarketId) -> u128 {
        let snap = self.borrows.get(&(*addr, market_id.clone()))
            .cloned()
            .unwrap_or_default();
        let index = self.configs.get(market_id)
            .map(|c| c.borrow_index)
            .unwrap_or(SCALE);
        snap.current_balance(index)
    }

    fn health_factor_after_borrow(
        &self,
        addr: &Address,
        borrow_market: &MarketId,
        additional_borrow: u128,
    ) -> u128 {
        // Collect all supplied markets for this account
        let supplied: Vec<(MarketId, u128)> = self.c_tokens.iter()
            .filter(|((a, _), _)| a == addr)
            .map(|((_, m), &ctok)| {
                let idx = self.configs.get(m).map(|c| c.borrow_index).unwrap_or(SCALE);
                let _ = idx;
                (m.clone(), ctok)
            })
            .collect();

        // Collect all borrows including the new one
        let mut borrow_map: HashMap<MarketId, u128> = HashMap::new();
        for ((a, m), snap) in &self.borrows {
            if a == addr {
                let idx = self.configs.get(m).map(|c| c.borrow_index).unwrap_or(SCALE);
                *borrow_map.entry(m.clone()).or_default() += snap.current_balance(idx);
            }
        }
        *borrow_map.entry(borrow_market.clone()).or_default() += additional_borrow;
        let borrows: Vec<(MarketId, u128)> = borrow_map.into_iter().collect();

        self.collateral.health_factor(addr, &supplied, &borrows, &self.prices)
    }

    // ── Public operations ──────────────────────────────────────────────────

    /// Supply `amount` of `market_id` tokens, receive cTokens.
    pub fn supply(
        &mut self,
        depositor: Address,
        market_id: &MarketId,
        amount: u128,
        current_block: u64,
    ) -> Result<u128, SbError> {
        if amount == 0 { return Err(SbError::ZeroAmount); }
        self.accrue(market_id, current_block);

        let mkt = self.registries.market(market_id)
            .ok_or_else(|| SbError::MarketNotFound(market_id.0.clone()))?;

        // cToken = amount × total_ctokens / total_underlying (1:1 on first)
        let c_tokens = if mkt.total_supply == 0 {
            amount
        } else {
            // Approximate: cToken per asset using exchange rate
            amount * SCALE / mkt.exchange_rate()
        };

        // Update market totals (read again to get mutable ref)
        // (in a real impl, MarketRegistry would be mutable)
        *self.c_tokens.entry((depositor, market_id.clone())).or_default() += c_tokens;
        Ok(c_tokens)
    }

    /// Borrow `amount` of `market_id` against supplied collateral.
    pub fn borrow(
        &mut self,
        borrower: Address,
        market_id: &MarketId,
        amount: u128,
        current_block: u64,
    ) -> Result<(), SbError> {
        if amount == 0 { return Err(SbError::ZeroAmount); }
        self.accrue(market_id, current_block);

        let cfg = self.configs.get(market_id)
            .ok_or_else(|| SbError::MarketNotFound(market_id.0.clone()))?;

        // Borrow cap check
        let total_borrows = self.registries.market(market_id)
            .map(|m| m.total_borrows)
            .unwrap_or(0);
        if cfg.borrow_cap > 0 && total_borrows + amount > cfg.borrow_cap {
            return Err(SbError::BorrowCapExceeded {
                cap: cfg.borrow_cap,
                current: total_borrows,
                amount,
            });
        }

        // Health factor check after hypothetical borrow
        let hf_after = self.health_factor_after_borrow(&borrower, market_id, amount);
        if hf_after < MIN_HEALTH_FACTOR {
            return Err(SbError::HealthFactorTooLow { hf_after });
        }

        let borrow_index = cfg.borrow_index;
        let snap = self.borrows
            .entry((borrower, market_id.clone()))
            .or_default();

        // Update snapshot: accrue existing principal to current index, then add new borrow.
        let current_principal = snap.current_balance(borrow_index);
        snap.principal      = current_principal + amount;
        snap.index_snapshot = borrow_index;
        Ok(())
    }

    /// Repay `amount` of borrowed `market_id` tokens.
    pub fn repay(
        &mut self,
        borrower: Address,
        market_id: &MarketId,
        amount: u128,
        current_block: u64,
    ) -> Result<u128, SbError> {
        if amount == 0 { return Err(SbError::ZeroAmount); }
        self.accrue(market_id, current_block);

        let borrow_index = self.configs.get(market_id)
            .map(|c| c.borrow_index)
            .unwrap_or(SCALE);

        let snap = self.borrows
            .entry((borrower, market_id.clone()))
            .or_default();
        let owed = snap.current_balance(borrow_index);
        let repay = amount.min(owed);
        let remaining = owed.saturating_sub(repay);
        snap.principal      = remaining;
        snap.index_snapshot = borrow_index;
        Ok(repay)
    }

    /// Redeem `c_token_amount` cTokens for underlying assets.
    pub fn redeem(
        &mut self,
        redeemer: Address,
        market_id: &MarketId,
        c_token_amount: u128,
        current_block: u64,
    ) -> Result<u128, SbError> {
        if c_token_amount == 0 { return Err(SbError::ZeroAmount); }
        self.accrue(market_id, current_block);

        let have = self.c_tokens
            .get(&(redeemer, market_id.clone()))
            .copied()
            .unwrap_or(0);
        if have < c_token_amount {
            return Err(SbError::InsufficientCTokens { have, need: c_token_amount });
        }
        *self.c_tokens.get_mut(&(redeemer, market_id.clone())).unwrap() -= c_token_amount;
        // Underlying returned = cTokens × exchange_rate / 1e18 (simplified 1:1 here)
        Ok(c_token_amount)
    }

    /// Liquidate an under-collateralised position.
    pub fn liquidate(
        &mut self,
        params:        &LiquidationParams,
        current_block: u64,
    ) -> Result<u128, SbError> {
        self.accrue(&params.repay_market, current_block);
        self.accrue(&params.collateral_market, current_block);

        // Compute health factor of the borrower
        let borrow_idx = self.configs.get(&params.repay_market)
            .map(|c| c.borrow_index)
            .unwrap_or(SCALE);
        let total_debt = self.borrows
            .get(&(params.borrower, params.repay_market.clone()))
            .map(|s| s.current_balance(borrow_idx))
            .unwrap_or(0);

        if total_debt == 0 { return Err(SbError::NotLiquidatable); }

        // Close factor: at most 50% of debt
        let max_repay = total_debt * CLOSE_FACTOR / SCALE;
        if params.repay_amount > max_repay {
            return Err(SbError::ExceedsCloseFactor {
                repay: params.repay_amount,
                limit: max_repay,
            });
        }

        let repay_price = self.prices.get(&params.repay_market).copied().unwrap_or(SCALE);
        let collateral_price = self.prices.get(&params.collateral_market).copied().unwrap_or(SCALE);

        // HF check: position must be unhealthy
        let hf = self.health_factor_after_borrow(&params.borrower, &params.repay_market, 0);
        if hf >= SCALE {
            return Err(SbError::NotLiquidatable);
        }

        let bonus = SCALE + 50_000_000_000_000_000u128; // 5% liquidation bonus
        let seized = compute_seized_collateral(
            params.repay_amount,
            repay_price,
            collateral_price,
            bonus,
        );

        // Reduce borrower's debt
        let snap = self.borrows
            .entry((params.borrower, params.repay_market.clone()))
            .or_default();
        let owed = snap.current_balance(borrow_idx);
        snap.principal      = owed.saturating_sub(params.repay_amount);
        snap.index_snapshot = borrow_idx;

        // Transfer seized collateral cTokens from borrower to liquidator
        let borrower_col = self.c_tokens
            .entry((params.borrower, params.collateral_market.clone()))
            .or_default();
        *borrower_col = borrower_col.saturating_sub(seized);
        *self.c_tokens
            .entry((params.liquidator, params.collateral_market.clone()))
            .or_default() += seized;

        Ok(seized)
    }

    pub fn c_token_balance(&self, addr: &Address, market: &MarketId) -> u128 {
        self.c_tokens.get(&(*addr, market.clone())).copied().unwrap_or(0)
    }

    pub fn borrow_balance(&self, addr: &Address, market: &MarketId) -> u128 {
        self.current_borrow_balance(addr, market)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collateral::CollateralFactor;

    fn addr(v: u8) -> Address { Address([v; 20]) }
    fn mid(s: &str) -> MarketId { MarketId(s.to_string()) }

    fn engine_with_market(
        market_id: &str,
        supply_liq: u128,
        price: u128,
        cf: u8,
    ) -> SupplyBorrowEngine {
        let mut eng = SupplyBorrowEngine::new();
        let m = mid(market_id);
        eng.add_market(
            Market::new(m.clone(), 1000),
            MarketConfig::new(m.clone()),
        );
        eng.set_price(m.clone(), price);
        eng.set_collateral_factor(CollateralFactor::new(m.clone(), cf, cf + 5));
        let _ = supply_liq;
        eng
    }

    #[test]
    fn supply_and_redeem_roundtrip() {
        let mut eng = engine_with_market("ZBX", 0, SCALE, 75);
        let ctok = eng.supply(addr(1), &mid("ZBX"), 500_000, 1).unwrap();
        assert!(ctok > 0);
        let underlying = eng.redeem(addr(1), &mid("ZBX"), ctok, 2).unwrap();
        assert_eq!(underlying, ctok);
    }

    #[test]
    fn zero_amount_rejected() {
        let mut eng = engine_with_market("ZBX", 0, SCALE, 75);
        assert!(matches!(eng.supply(addr(1), &mid("ZBX"), 0, 1), Err(SbError::ZeroAmount)));
        assert!(matches!(eng.borrow(addr(1), &mid("ZBX"), 0, 1), Err(SbError::ZeroAmount)));
        assert!(matches!(eng.repay(addr(1), &mid("ZBX"), 0, 1), Err(SbError::ZeroAmount)));
    }

    #[test]
    fn borrow_cap_enforced() {
        let mut eng = engine_with_market("ZBX", 0, SCALE, 75);
        eng.configs.get_mut(&mid("ZBX")).unwrap().borrow_cap = 100;
        // Simulate deposited collateral
        *eng.c_tokens.entry((addr(1), mid("ZBX"))).or_default() = 1_000_000;
        let err = eng.borrow(addr(1), &mid("ZBX"), 200, 1).unwrap_err();
        assert!(matches!(err, SbError::BorrowCapExceeded { .. }));
    }

    #[test]
    fn repay_reduces_debt() {
        let mut eng = engine_with_market("ZBX", 0, SCALE, 75);
        // Give borrower lots of collateral so health factor passes
        *eng.c_tokens.entry((addr(1), mid("ZBX"))).or_default() = u128::MAX / 2;
        // Health factor will be max (u128::MAX) since collateral >> borrow
        // Force borrow by directly inserting snapshot
        eng.borrows.insert((addr(1), mid("ZBX")), BorrowSnapshot {
            principal: 1_000,
            index_snapshot: SCALE,
        });
        let repaid = eng.repay(addr(1), &mid("ZBX"), 400, 1).unwrap();
        assert_eq!(repaid, 400);
        assert_eq!(eng.borrow_balance(&addr(1), &mid("ZBX")), 600);
    }

    #[test]
    fn redeem_insufficient_ctokens_rejected() {
        let mut eng = engine_with_market("ZBX", 0, SCALE, 75);
        eng.supply(addr(1), &mid("ZBX"), 100, 1).unwrap();
        assert!(matches!(eng.redeem(addr(1), &mid("ZBX"), 10_000, 2), Err(SbError::InsufficientCTokens { .. })));
    }
}
