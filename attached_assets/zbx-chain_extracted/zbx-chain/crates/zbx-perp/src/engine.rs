//! PerpEngine — top-level coordinator that wires together market, position,
//! order, funding, and liquidation subsystems.
//!
//! This is the single entry-point that the block executor (and tx_handler)
//! should call.  All oracle price resolution happens here via a pluggable
//! `OracleProvider` trait, so tests can inject mock prices without a live node.

use zbx_types::address::Address;
use crate::error::PerpError;
use crate::funding::settle_funding;
use crate::liquidation::{liquidate, liquidate_cross};
use crate::market::MarketRegistry;
use crate::order::{
    set_stop_loss, set_take_profit, set_trailing_stop,
    trigger_order, trigger_stop_loss, trigger_take_profit, update_trailing_stop,
};
use crate::position::PositionStore;
use crate::types::{
    CloseResult, CrossAccountView, LiquidationResult, MarketView, OpenPositionParams,
    OpenPositionResult, PositionView,
};
use crate::FUNDING_INTERVAL;

// ─── Oracle trait ─────────────────────────────────────────────────────────────

/// Abstraction over oracle price resolution.
/// In production the executor provides a Chainlink-compatible reader.
/// In tests a simple HashMap shim is sufficient.
pub trait OracleProvider: Send + Sync {
    /// Return the current mark price for a market (18-decimal wei).
    /// Returns `None` if the oracle is unavailable or the price is stale.
    fn mark_price(&self, oracle_addr: Address) -> Option<u128>;
}

// ─── PerpEngine ───────────────────────────────────────────────────────────────

/// Top-level engine.
pub struct PerpEngine {
    pub markets:   MarketRegistry,
    pub positions: PositionStore,
    owner:         Address,
    oracle:        Box<dyn OracleProvider>,
}

impl PerpEngine {
    /// Construct a new engine with a given owner and oracle provider.
    pub fn new(owner: Address, oracle: Box<dyn OracleProvider>) -> Self {
        Self {
            markets:   MarketRegistry::new(),
            positions: PositionStore::new(),
            owner,
            oracle,
        }
    }

    // ── Owner-only ─────────────────────────────────────────────────────────

    /// Add a new trading pair. Only callable by `owner`.
    pub fn add_market(
        &mut self,
        caller: Address,
        oracle_addr: Address,
        symbol: String,
        max_leverage: u64,
        now: u64,
    ) -> Result<u64, PerpError> {
        self.require_owner(caller)?;
        self.markets.add_market(oracle_addr, symbol, max_leverage, now)
    }

    /// Update an existing market's oracle / active flag / leverage cap.
    pub fn update_market(
        &mut self,
        caller: Address,
        market_id: u64,
        oracle_addr: Address,
        active: bool,
        max_leverage: u64,
    ) -> Result<(), PerpError> {
        self.require_owner(caller)?;
        self.markets.update_market(market_id, oracle_addr, active, max_leverage)
    }

    // ── Position lifecycle ──────────────────────────────────────────────────

    /// Open a new position.
    #[allow(clippy::too_many_arguments)]
    pub fn open_position(
        &mut self,
        sender:    Address,
        market_id: u64,
        is_long:   bool,
        collateral: u128,
        leverage:  u64,
        is_cross:  bool,
        sl_price:  u128,
        tp_price:  u128,
        now:       u64,
    ) -> Result<OpenPositionResult, PerpError> {
        let market = self.markets.get(market_id)?;
        if !market.active { return Err(PerpError::MarketInactive(market_id)); }
        let oracle_addr = market.oracle;

        let mark = self.resolve_price(market_id, oracle_addr)?;

        // Settle funding before any trade
        {
            let m = self.markets.get_mut(market_id)?;
            settle_funding(m, now, FUNDING_INTERVAL);
        }

        // Validate SL/TP against mark price
        if sl_price != 0 { crate::order::validate_sl(is_long, mark, sl_price)?; }
        if tp_price != 0 { crate::order::validate_tp(is_long, mark, tp_price)?; }

        let params = OpenPositionParams {
            market_id, is_long, collateral, leverage, is_cross, sl_price, tp_price,
        };
        let market = self.markets.get_mut(market_id)?;
        self.positions.open(sender, &params, mark, market)
    }

    /// Fully close a position.
    pub fn close_position(
        &mut self,
        sender: Address,
        pos_id: u64,
        now:    u64,
    ) -> Result<CloseResult, PerpError> {
        let (market_id, oracle_addr) = self.pos_market_oracle(pos_id)?;
        let mark = self.resolve_price(market_id, oracle_addr)?;
        self.settle(market_id, now)?;
        let market = self.markets.get_mut(market_id)?;
        self.positions.close(sender, pos_id, mark, market)
    }

    /// Partially close a position.
    pub fn partial_close(
        &mut self,
        sender:    Address,
        pos_id:    u64,
        close_bps: u64,
        now:       u64,
    ) -> Result<CloseResult, PerpError> {
        let (market_id, oracle_addr) = self.pos_market_oracle(pos_id)?;
        let mark = self.resolve_price(market_id, oracle_addr)?;
        self.settle(market_id, now)?;
        let market = self.markets.get_mut(market_id)?;
        self.positions.partial_close(sender, pos_id, close_bps, mark, market)
    }

    /// Add collateral to an isolated position.
    pub fn add_collateral(&mut self, pos_id: u64, amount: u128) -> Result<(), PerpError> {
        self.positions.add_collateral(pos_id, amount)
    }

    // ── SL / TP / Trailing stop ─────────────────────────────────────────────

    pub fn set_stop_loss(
        &mut self, sender: Address, pos_id: u64, sl_price: u128, now: u64,
    ) -> Result<(), PerpError> {
        let (market_id, oracle_addr) = self.pos_market_oracle(pos_id)?;
        let mark = self.resolve_price(market_id, oracle_addr)?;
        _ = now;
        set_stop_loss(&mut self.positions, sender, pos_id, sl_price, mark)
    }

    pub fn set_take_profit(
        &mut self, sender: Address, pos_id: u64, tp_price: u128, now: u64,
    ) -> Result<(), PerpError> {
        let (market_id, oracle_addr) = self.pos_market_oracle(pos_id)?;
        let mark = self.resolve_price(market_id, oracle_addr)?;
        _ = now;
        set_take_profit(&mut self.positions, sender, pos_id, tp_price, mark)
    }

    pub fn set_trailing_stop(
        &mut self, sender: Address, pos_id: u64, trail_bps: u64, now: u64,
    ) -> Result<(), PerpError> {
        let (market_id, oracle_addr) = self.pos_market_oracle(pos_id)?;
        let mark = self.resolve_price(market_id, oracle_addr)?;
        _ = now;
        set_trailing_stop(&mut self.positions, sender, pos_id, trail_bps, mark)
    }

    pub fn update_trailing_stop(&mut self, pos_id: u64, now: u64) -> Result<u128, PerpError> {
        let (market_id, oracle_addr) = self.pos_market_oracle(pos_id)?;
        let mark = self.resolve_price(market_id, oracle_addr)?;
        _ = now;
        update_trailing_stop(&mut self.positions, pos_id, mark)
    }

    // ── Keeper triggers ─────────────────────────────────────────────────────

    pub fn trigger_order(
        &mut self, keeper: Address, pos_id: u64, now: u64,
    ) -> Result<(CloseResult, u128), PerpError> {
        let (market_id, oracle_addr) = self.pos_market_oracle(pos_id)?;
        let mark = self.resolve_price(market_id, oracle_addr)?;
        self.settle(market_id, now)?;
        let market = self.markets.get_mut(market_id)?;
        trigger_order(&mut self.positions, keeper, pos_id, mark, market)
    }

    pub fn trigger_stop_loss(
        &mut self, keeper: Address, pos_id: u64, now: u64,
    ) -> Result<(CloseResult, u128), PerpError> {
        let (market_id, oracle_addr) = self.pos_market_oracle(pos_id)?;
        let mark = self.resolve_price(market_id, oracle_addr)?;
        self.settle(market_id, now)?;
        let market = self.markets.get_mut(market_id)?;
        trigger_stop_loss(&mut self.positions, keeper, pos_id, mark, market)
    }

    pub fn trigger_take_profit(
        &mut self, keeper: Address, pos_id: u64, now: u64,
    ) -> Result<(CloseResult, u128), PerpError> {
        let (market_id, oracle_addr) = self.pos_market_oracle(pos_id)?;
        let mark = self.resolve_price(market_id, oracle_addr)?;
        self.settle(market_id, now)?;
        let market = self.markets.get_mut(market_id)?;
        trigger_take_profit(&mut self.positions, keeper, pos_id, mark, market)
    }

    // ── Liquidation ─────────────────────────────────────────────────────────

    pub fn liquidate(
        &mut self, keeper: Address, pos_id: u64, now: u64,
    ) -> Result<LiquidationResult, PerpError> {
        let (market_id, oracle_addr) = self.pos_market_oracle(pos_id)?;
        let mark = self.resolve_price(market_id, oracle_addr)?;
        self.settle(market_id, now)?;
        let market = self.markets.get_mut(market_id)?;
        liquidate(&mut self.positions, keeper, pos_id, mark, market)
    }

    pub fn liquidate_cross(
        &mut self, keeper: Address, trader: Address, now: u64,
    ) -> Result<LiquidationResult, PerpError> {
        _ = now;
        // Build a closure that resolves mark prices for each cross position's market
        let market_count = self.markets.market_count();
        let oracle = &self.oracle;
        let markets_ref = &self.markets;
        let mut prices: std::collections::HashMap<u64, u128> = std::collections::HashMap::new();
        for mid in 0..market_count {
            if let Ok(m) = markets_ref.get(mid) {
                if let Some(p) = oracle.mark_price(m.oracle) {
                    prices.insert(mid, p);
                }
            }
        }
        liquidate_cross(&mut self.positions, keeper, trader, &mut |mid| prices.get(&mid).copied())
    }

    // ── Cross margin ──────────────────────────────────────────────────────

    pub fn deposit_cross(&mut self, sender: Address, amount: u128) -> Result<(), PerpError> {
        self.positions.deposit_cross(sender, amount)
    }

    pub fn withdraw_cross(&mut self, sender: Address, amount: u128) -> Result<(), PerpError> {
        self.positions.withdraw_cross(sender, amount)
    }

    // ── Funding ────────────────────────────────────────────────────────────

    pub fn update_funding(&mut self, market_id: u64, now: u64) -> Result<u64, PerpError> {
        if market_id >= self.markets.market_count() {
            return Err(PerpError::MarketNotFound(market_id));
        }
        Ok(self.markets.settle_funding(market_id, now)?)
    }

    // ── Views ──────────────────────────────────────────────────────────────

    pub fn market_view(&self, market_id: u64, now: u64) -> Result<MarketView, PerpError> {
        let m = self.markets.get(market_id)?;
        let mark = self.oracle.mark_price(m.oracle)
            .ok_or(PerpError::ZeroOraclePrice(market_id))?;
        self.markets.view(market_id, mark, now)
    }

    pub fn all_market_views(&self, now: u64) -> Vec<Result<MarketView, PerpError>> {
        (0..self.markets.market_count())
            .map(|id| self.market_view(id, now))
            .collect()
    }

    pub fn position_view(&self, pos_id: u64) -> Option<PositionView> {
        let p = self.positions.get_position(pos_id)?;
        let market = self.markets.get(p.market_id).ok()?;
        let mark   = self.oracle.mark_price(market.oracle)?;
        self.positions.view(pos_id, mark, market)
    }

    pub fn cross_account_view(&self, trader: Address, now: u64) -> CrossAccountView {
        _ = now;
        let pos_ids  = self.positions.cross_position_ids(trader);
        let balance  = self.positions.cross_balance(trader);
        let maint    = self.positions.cross_maint_margin(trader);
        let free     = self.positions.free_cross_margin(trader);
        let liq_thr  = maint;
        let liquidatable = (balance as i128) < maint as i128;

        // Equity = balance + sum(unrealised PnL for all open cross positions)
        let mut unrealised_sum: i128 = 0;
        for pid in &pos_ids {
            let Some(p) = self.positions.get_position(*pid) else { continue };
            if p.closed { continue; }
            let m = self.markets.get(p.market_id);
            if let Ok(mkt) = m {
                if let Some(mark) = self.oracle.mark_price(mkt.oracle) {
                    unrealised_sum = unrealised_sum
                        .saturating_add(self.positions.unrealised_pnl(*pid, mark));
                }
            }
        }
        let equity = (balance as i128).saturating_add(unrealised_sum);

        CrossAccountView {
            trader, balance, equity, maint_margin: maint,
            free_margin: free, liq_threshold: liq_thr, liquidatable,
            position_ids: pos_ids,
        }
    }

    pub fn protocol_fee_balance(&self) -> u128 {
        self.positions.protocol_fee_balance
    }

    pub fn market_count(&self) -> u64 {
        self.markets.market_count()
    }

    pub fn next_position_id(&self) -> u64 {
        self.positions.next_position_id()
    }

    // ── Private helpers ────────────────────────────────────────────────────

    fn require_owner(&self, caller: Address) -> Result<(), PerpError> {
        if caller != self.owner { Err(PerpError::NotOwner) } else { Ok(()) }
    }

    fn pos_market_oracle(&self, pos_id: u64) -> Result<(u64, Address), PerpError> {
        let p = self.positions.get_position(pos_id)
            .ok_or(PerpError::PositionNotFound(pos_id))?;
        let m = self.markets.get(p.market_id)?;
        Ok((p.market_id, m.oracle))
    }

    fn resolve_price(&self, market_id: u64, oracle_addr: Address) -> Result<u128, PerpError> {
        self.oracle.mark_price(oracle_addr)
            .ok_or(PerpError::ZeroOraclePrice(market_id))
    }

    fn settle(&mut self, market_id: u64, now: u64) -> Result<(), PerpError> {
        self.markets.settle_funding(market_id, now)?;
        Ok(())
    }
}

// ─── Mock oracle for tests ────────────────────────────────────────────────────

#[cfg(test)]
pub struct MockOracle(pub std::collections::HashMap<[u8; 20], u128>);

#[cfg(test)]
impl OracleProvider for MockOracle {
    fn mark_price(&self, addr: Address) -> Option<u128> {
        self.0.get(&addr.0).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbx_types::address::Address;

    const ONE: u128 = 1_000_000_000_000_000_000;
    fn addr(b: u8) -> Address { Address([b; 20]) }
    fn oracle_addr() -> Address { addr(50) }

    fn make_engine(mark: u128) -> PerpEngine {
        let mut prices = std::collections::HashMap::new();
        prices.insert(oracle_addr().0, mark);
        PerpEngine::new(addr(0), Box::new(MockOracle(prices)))
    }

    fn setup_market(engine: &mut PerpEngine) -> u64 {
        engine.add_market(addr(0), oracle_addr(), "BTC".into(), 200, 0).unwrap()
    }

    #[test]
    fn open_and_close_isolated_long() {
        let mut eng = make_engine(50_000 * ONE);
        let mid = setup_market(&mut eng);
        let res = eng.open_position(addr(1), mid, true, 100 * ONE, 10, false, 0, 0, 0).unwrap();
        assert!(res.size > 0);
        let cr = eng.close_position(addr(1), res.position_id, 0).unwrap();
        // Price unchanged → PnL = 0 (minus fees)
        assert!(cr.payout <= 100 * ONE); // fee deducted
    }

    #[test]
    fn only_owner_can_add_market() {
        let mut eng = make_engine(50_000 * ONE);
        let err = eng.add_market(addr(99), oracle_addr(), "X".into(), 10, 0).unwrap_err();
        assert_eq!(err, PerpError::NotOwner);
    }

    #[test]
    fn liquidate_healthy_fails() {
        let mut eng = make_engine(50_000 * ONE);
        let mid = setup_market(&mut eng);
        let res = eng.open_position(addr(1), mid, true, 100 * ONE, 10, false, 0, 0, 0).unwrap();
        // Price unchanged → not liquidatable
        let err = eng.liquidate(addr(99), res.position_id, 0).unwrap_err();
        assert_eq!(err, PerpError::NotLiquidatable(res.position_id));
    }

    #[test]
    fn trigger_order_requires_sl_hit() {
        let mut eng = make_engine(50_000 * ONE);
        let mid = setup_market(&mut eng);
        let res = eng.open_position(addr(1), mid, true, 100 * ONE, 10, false, 0, 0, 0).unwrap();
        // No SL set — neither triggered
        let err = eng.trigger_order(addr(99), res.position_id, 0).unwrap_err();
        assert_eq!(err, PerpError::NeitherTriggered(res.position_id));
    }

    #[test]
    fn deposit_and_withdraw_cross() {
        let mut eng = make_engine(1_000 * ONE);
        eng.deposit_cross(addr(1), 500 * ONE).unwrap();
        assert_eq!(eng.positions.cross_balance(addr(1)), 500 * ONE);
        eng.withdraw_cross(addr(1), 200 * ONE).unwrap();
        assert_eq!(eng.positions.cross_balance(addr(1)), 300 * ONE);
    }
}
