//! Position management — open, close, partial-close, add-collateral.
//!
//! This module mirrors the core position lifecycle from ZbxPerpetuals.sol:
//! `openPosition`, `closePosition`, `partialClose`, `addCollateral`.
//!
//! The caller (PerpEngine) is responsible for:
//!   - Resolving the oracle price before calling these functions.
//!   - Settling funding via `market::settle_funding` before any trade.
//!   - Validating SL/TP via `order::validate_sl` / `order::validate_tp`.
//!   - Applying balance transfers (ERC-20 equivalent) after the call.

use std::collections::HashMap;
use zbx_types::address::Address;
use crate::error::PerpError;
use crate::funding::funding_cost_for_position;
use crate::types::{
    CloseResult, CrossAccount, Market, OpenPositionParams, OpenPositionResult, Position,
    PositionView,
};
use crate::{
    KEEPER_BOUNTY_BPS, LIQUIDATION_BOUNTY_BPS, MAINTENANCE_MARGIN_BPS, PROTOCOL_FEE_BPS,
};

/// In-memory position store + cross-account ledger.
#[derive(Debug, Default)]
pub struct PositionStore {
    pub(crate) positions:       HashMap<u64, Position>,
    pub(crate) cross_accounts:  HashMap<Address, CrossAccount>,
    next_pos_id:                u64,
    /// Total collected protocol fees (in collateral wei).
    pub protocol_fee_balance:   u128,
}

impl PositionStore {
    pub fn new() -> Self { Self::default() }

    // ── Open ──────────────────────────────────────────────────────────────

    /// Open a new position.
    ///
    /// For isolated positions the caller must have already transferred `params.collateral`
    /// tokens into the contract before this call.  For cross positions the balance
    /// is deducted in-place from the cross account.
    pub fn open(
        &mut self,
        sender: Address,
        params: &OpenPositionParams,
        mark_price: u128,
        market: &mut Market,
    ) -> Result<OpenPositionResult, PerpError> {
        if params.collateral == 0 { return Err(PerpError::ZeroCollateral); }
        if params.leverage == 0   { return Err(PerpError::ZeroLeverage); }
        if params.leverage > market.max_leverage {
            return Err(PerpError::LeverageTooHigh {
                got: params.leverage,
                max: market.max_leverage,
            });
        }
        if mark_price == 0 { return Err(PerpError::ZeroOraclePrice(params.market_id)); }

        let fee    = (params.collateral * PROTOCOL_FEE_BPS as u128) / 10_000;
        let col_net = params.collateral.saturating_sub(fee);
        let size   = col_net.saturating_mul(params.leverage as u128);

        if params.is_cross {
            let ca = self.cross_accounts.entry(sender).or_default();
            let maint_existing = self.maint_margin_for_cross_inner(sender);
            let new_maint = (size * MAINTENANCE_MARGIN_BPS as u128) / 10_000;
            let needed = params.collateral
                .saturating_add(maint_existing)
                .saturating_add(new_maint);
            if ca.balance < needed {
                return Err(PerpError::InsufficientCrossMargin {
                    have: ca.balance,
                    need: needed,
                });
            }
            ca.balance = ca.balance.saturating_sub(fee);
            ca.initial_margin = ca.initial_margin.saturating_add(col_net);
        }

        self.protocol_fee_balance = self.protocol_fee_balance.saturating_add(fee);

        self.next_pos_id += 1;
        let pos_id = self.next_pos_id;

        let position = Position {
            trader: sender,
            market_id: params.market_id,
            is_long: params.is_long,
            is_cross: params.is_cross,
            collateral: if params.is_cross { 0 } else { col_net },
            size,
            entry_price: mark_price,
            funding_entry_rate: market.cumulative_funding,
            stop_loss: params.sl_price,
            take_profit: params.tp_price,
            trail_bps: 0,
            trail_peak: mark_price,
            closed: false,
            initial_margin: col_net,
        };

        if params.is_long {
            market.total_long_oi = market.total_long_oi.saturating_add(size);
        } else {
            market.total_short_oi = market.total_short_oi.saturating_add(size);
        }

        if params.is_cross {
            let ca = self.cross_accounts.entry(sender).or_default();
            ca.pos_ids.push(pos_id);
        }

        self.positions.insert(pos_id, position);

        tracing::info!(
            pos_id, trader = ?sender, market_id = params.market_id,
            is_long = params.is_long, size, "position opened"
        );

        Ok(OpenPositionResult {
            position_id: pos_id,
            size,
            entry_price: mark_price,
            fee_charged: fee,
        })
    }

    // ── Full close ────────────────────────────────────────────────────────

    /// Fully close a position. Caller must be the position owner.
    /// Returns `CloseResult` — caller handles the token transfer.
    pub fn close(
        &mut self,
        sender: Address,
        pos_id: u64,
        exit_price: u128,
        market: &mut Market,
    ) -> Result<CloseResult, PerpError> {
        self.assert_owner(sender, pos_id)?;
        let p = self.positions.get(&pos_id).ok_or(PerpError::PositionNotFound(pos_id))?;
        if p.closed { return Err(PerpError::AlreadyClosed(pos_id)); }

        let (net_pnl, payout, fee) = self.execute_close_inner(pos_id, exit_price, market)?;

        tracing::info!(pos_id, trader = ?sender, net_pnl, payout, "position closed");
        Ok(CloseResult { exit_price, net_pnl, payout, fee })
    }

    /// Close a position without checking ownership (keeper liquidation / trigger path).
    pub(crate) fn close_unchecked(
        &mut self,
        pos_id: u64,
        exit_price: u128,
        market: &mut Market,
    ) -> Result<CloseResult, PerpError> {
        let (net_pnl, payout, fee) = self.execute_close_inner(pos_id, exit_price, market)?;
        Ok(CloseResult { exit_price, net_pnl, payout, fee })
    }

    // ── Partial close ──────────────────────────────────────────────────────

    /// Partially close a position by `close_bps` basis points (1–10000).
    /// 10000 = full close. Caller must be the position owner.
    pub fn partial_close(
        &mut self,
        sender: Address,
        pos_id: u64,
        close_bps: u64,
        exit_price: u128,
        market: &mut Market,
    ) -> Result<CloseResult, PerpError> {
        if close_bps == 0 || close_bps > 10_000 { return Err(PerpError::InvalidBps); }
        self.assert_owner(sender, pos_id)?;

        if close_bps == 10_000 {
            return self.close(sender, pos_id, exit_price, market);
        }

        let p = self.positions.get(&pos_id).ok_or(PerpError::PositionNotFound(pos_id))?;
        if p.closed { return Err(PerpError::AlreadyClosed(pos_id)); }

        let close_size = (p.size * close_bps as u128) / 10_000;
        let close_col  = if p.is_cross { 0 } else { (p.collateral * close_bps as u128) / 10_000 };
        let is_long    = p.is_long;
        let is_cross   = p.is_cross;
        let entry      = p.entry_price;
        let funding_er = p.funding_entry_rate;

        let pnl    = pnl_for(is_long, entry, exit_price, close_size);
        let fund   = funding_cost_for_position(market.cumulative_funding, funding_er, close_size);
        let net_pnl = pnl.saturating_sub(fund);

        let p = self.positions.get_mut(&pos_id).unwrap();
        p.size = p.size.saturating_sub(close_size);
        if !is_cross { p.collateral = p.collateral.saturating_sub(close_col); }

        if is_long {
            market.total_long_oi = market.total_long_oi.saturating_sub(close_size);
        } else {
            market.total_short_oi = market.total_short_oi.saturating_sub(close_size);
        }

        let (payout, fee) = if is_cross {
            self.settle_cross_pnl(sender, net_pnl);
            let im_rel = (p.initial_margin * close_bps as u128) / 10_000;
            let ca = self.cross_accounts.entry(sender).or_default();
            ca.initial_margin = ca.initial_margin.saturating_sub(im_rel);
            p.initial_margin = p.initial_margin.saturating_sub(im_rel);
            (0u128, 0u128)
        } else {
            let raw_payout = if net_pnl >= 0 {
                close_col.saturating_add(net_pnl.unsigned_abs())
            } else {
                let loss = net_pnl.unsigned_abs();
                if loss >= close_col { 0 } else { close_col - loss }
            };
            let fee = (raw_payout * PROTOCOL_FEE_BPS as u128) / 10_000;
            self.protocol_fee_balance = self.protocol_fee_balance.saturating_add(fee);
            (raw_payout.saturating_sub(fee), fee)
        };

        tracing::info!(pos_id, trader = ?sender, close_bps, net_pnl, payout, "partial close");
        Ok(CloseResult { exit_price, net_pnl, payout, fee })
    }

    // ── Add collateral (isolated) ─────────────────────────────────────────

    /// Add collateral to an isolated position (push liquidation price away from entry).
    /// Reverts for cross positions — they use `deposit_cross` instead.
    pub fn add_collateral(
        &mut self,
        pos_id: u64,
        amount: u128,
    ) -> Result<(), PerpError> {
        if amount == 0 { return Err(PerpError::ZeroAmount); }
        let p = self.positions.get_mut(&pos_id).ok_or(PerpError::PositionNotFound(pos_id))?;
        if p.closed  { return Err(PerpError::AlreadyClosed(pos_id)); }
        if p.is_cross { return Err(PerpError::NotIsolatedPosition); }
        p.collateral = p.collateral.saturating_add(amount);
        tracing::info!(pos_id, amount, "collateral added");
        Ok(())
    }

    // ── Cross account ──────────────────────────────────────────────────────

    /// Deposit into a cross-margin account (caller has already transferred tokens).
    pub fn deposit_cross(&mut self, trader: Address, amount: u128) -> Result<(), PerpError> {
        if amount == 0 { return Err(PerpError::ZeroAmount); }
        self.cross_accounts.entry(trader).or_default().balance =
            self.cross_accounts[&trader].balance.saturating_add(amount);
        Ok(())
    }

    /// Withdraw free margin from a cross-margin account.
    pub fn withdraw_cross(&mut self, trader: Address, amount: u128) -> Result<(), PerpError> {
        if amount == 0 { return Err(PerpError::ZeroAmount); }
        let free = self.free_cross_margin(trader);
        if amount > free {
            return Err(PerpError::CrossWithdrawTooLarge { free, requested: amount });
        }
        let ca = self.cross_accounts.entry(trader).or_default();
        ca.balance = ca.balance.saturating_sub(amount);
        Ok(())
    }

    /// Get free cross-margin available to a trader.
    pub fn free_cross_margin(&self, trader: Address) -> u128 {
        let Some(ca) = self.cross_accounts.get(&trader) else { return 0; };
        let maint = self.maint_margin_for_cross_inner(trader);
        if (ca.balance as i128) <= maint as i128 { 0 } else { ca.balance.saturating_sub(maint) }
    }

    /// Total maintenance margin owed by a cross account.
    pub fn cross_maint_margin(&self, trader: Address) -> u128 {
        self.maint_margin_for_cross_inner(trader)
    }

    pub fn cross_balance(&self, trader: Address) -> u128 {
        self.cross_accounts.get(&trader).map(|ca| ca.balance).unwrap_or(0)
    }

    pub fn cross_position_ids(&self, trader: Address) -> Vec<u64> {
        self.cross_accounts.get(&trader).map(|ca| ca.pos_ids.clone()).unwrap_or_default()
    }

    // ── Views ──────────────────────────────────────────────────────────────

    pub fn get_position(&self, pos_id: u64) -> Option<&Position> {
        self.positions.get(&pos_id)
    }

    /// Compute unrealised PnL for a position given the current mark price.
    pub fn unrealised_pnl(&self, pos_id: u64, mark_price: u128) -> i128 {
        let Some(p) = self.positions.get(&pos_id) else { return 0; };
        pnl_for(p.is_long, p.entry_price, mark_price, p.size)
    }

    /// Health in basis points (0 = liquidatable, 10000 = fully collateralised).
    /// Always 0 for cross or closed positions (use cross equity for those).
    pub fn health_bps(
        &self,
        pos_id: u64,
        mark_price: u128,
        market: &Market,
    ) -> u64 {
        let Some(p) = self.positions.get(&pos_id) else { return 0; };
        if p.closed || p.is_cross || p.collateral == 0 { return 0; }

        let pnl  = pnl_for(p.is_long, p.entry_price, mark_price, p.size);
        let fund = funding_cost_for_position(market.cumulative_funding, p.funding_entry_rate, p.size);
        let eq   = (p.collateral as i128).saturating_add(pnl).saturating_sub(fund);
        if eq <= 0 { return 0; }

        let maint = (p.size * MAINTENANCE_MARGIN_BPS as u128) / 10_000;
        if (eq as u128) <= maint { return 0; }

        let h = ((eq as u128) * 10_000) / p.collateral;
        h.min(10_000) as u64
    }

    /// Exact oracle price at which an isolated position gets liquidated.
    /// Returns 0 for cross or closed positions.
    ///
    /// Formula (from ZbxPerpetuals.sol):
    ///   LONG:  liq = entry + entry × (MM − col + funding) / size
    ///   SHORT: liq = entry − entry × (MM − col + funding) / size
    pub fn liquidation_price(
        &self,
        pos_id: u64,
        market: &Market,
    ) -> u128 {
        let Some(p) = self.positions.get(&pos_id) else { return 0; };
        if p.closed || p.is_cross || p.trader == Address([0u8; 20]) { return 0; }

        let funding = funding_cost_for_position(
            market.cumulative_funding, p.funding_entry_rate, p.size
        );
        let maint_margin = ((p.size * MAINTENANCE_MARGIN_BPS as u128) / 10_000) as i128;
        let numerator    = maint_margin
            .saturating_sub(p.collateral as i128)
            .saturating_add(funding);

        let entry_i = p.entry_price as i128;
        let size_i  = p.size as i128;
        if size_i == 0 { return 0; }
        let delta   = entry_i.saturating_mul(numerator).checked_div(size_i).unwrap_or(0);

        let liq: i128 = if p.is_long {
            entry_i.saturating_add(delta)
        } else {
            entry_i.saturating_sub(delta)
        };
        if liq > 0 { liq as u128 } else { 0 }
    }

    /// Build a full `PositionView` for a position + live mark price.
    pub fn view(
        &self,
        pos_id: u64,
        mark_price: u128,
        market: &Market,
    ) -> Option<PositionView> {
        let p = self.positions.get(&pos_id)?;
        let unrealised_pnl  = self.unrealised_pnl(pos_id, mark_price);
        let health          = self.health_bps(pos_id, mark_price, market);
        let liq_price       = self.liquidation_price(pos_id, market);
        let is_sl_triggered = !p.closed && p.stop_loss > 0
            && ((p.is_long && mark_price <= p.stop_loss)
                || (!p.is_long && mark_price >= p.stop_loss));
        let is_tp_triggered = !p.closed && p.take_profit > 0
            && ((p.is_long && mark_price >= p.take_profit)
                || (!p.is_long && mark_price <= p.take_profit));
        let is_liq = !p.closed && !p.is_cross && health == 0 && p.collateral > 0;

        Some(PositionView {
            position_id:        pos_id,
            trader:             p.trader,
            market_id:          p.market_id,
            is_long:            p.is_long,
            is_cross:           p.is_cross,
            collateral:         p.collateral,
            size:               p.size,
            entry_price:        p.entry_price,
            funding_entry_rate: p.funding_entry_rate,
            stop_loss:          p.stop_loss,
            take_profit:        p.take_profit,
            trail_bps:          p.trail_bps,
            trail_peak:         p.trail_peak,
            closed:             p.closed,
            initial_margin:     p.initial_margin,
            unrealised_pnl,
            health_bps:         health,
            liquidation_price:  liq_price,
            is_sl_triggered,
            is_tp_triggered,
            is_liquidatable:    is_liq,
        })
    }

    // ── Maintenance margin helpers ─────────────────────────────────────────

    fn maint_margin_for_cross_inner(&self, trader: Address) -> u128 {
        let Some(ca) = self.cross_accounts.get(&trader) else { return 0; };
        ca.pos_ids.iter().filter_map(|pid| {
            let p = self.positions.get(pid)?;
            if p.closed { return None; }
            Some((p.size * MAINTENANCE_MARGIN_BPS as u128) / 10_000)
        }).sum()
    }

    // ── Internal close helper ─────────────────────────────────────────────

    fn execute_close_inner(
        &mut self,
        pos_id: u64,
        exit_price: u128,
        market: &mut Market,
    ) -> Result<(i128, u128, u128), PerpError> {
        let p = self.positions.get(&pos_id).ok_or(PerpError::PositionNotFound(pos_id))?;
        if p.closed { return Err(PerpError::AlreadyClosed(pos_id)); }

        let pnl     = pnl_for(p.is_long, p.entry_price, exit_price, p.size);
        let fund    = funding_cost_for_position(market.cumulative_funding, p.funding_entry_rate, p.size);
        let net_pnl = pnl.saturating_sub(fund);
        let size    = p.size;
        let col     = p.collateral;
        let is_long = p.is_long;
        let is_cross = p.is_cross;
        let trader  = p.trader;
        let im      = p.initial_margin;

        if is_long {
            market.total_long_oi = market.total_long_oi.saturating_sub(size);
        } else {
            market.total_short_oi = market.total_short_oi.saturating_sub(size);
        }

        let p = self.positions.get_mut(&pos_id).unwrap();
        p.closed = true;

        let (payout, fee) = if is_cross {
            self.settle_cross_pnl(trader, net_pnl);
            let ca = self.cross_accounts.entry(trader).or_default();
            ca.initial_margin = ca.initial_margin.saturating_sub(im);
            let p2 = self.positions.get_mut(&pos_id).unwrap();
            p2.initial_margin = 0;
            self.remove_cross_pos_id(trader, pos_id);
            (0u128, 0u128)
        } else {
            let raw = if net_pnl >= 0 {
                col.saturating_add(net_pnl.unsigned_abs())
            } else {
                let loss = net_pnl.unsigned_abs();
                if loss >= col { 0 } else { col - loss }
            };
            let fee = (raw * PROTOCOL_FEE_BPS as u128) / 10_000;
            self.protocol_fee_balance = self.protocol_fee_balance.saturating_add(fee);
            (raw.saturating_sub(fee), fee)
        };

        Ok((net_pnl, payout, fee))
    }

    fn settle_cross_pnl(&mut self, trader: Address, net_pnl: i128) {
        let ca = self.cross_accounts.entry(trader).or_default();
        if net_pnl >= 0 {
            ca.balance = ca.balance.saturating_add(net_pnl.unsigned_abs());
        } else {
            let loss = net_pnl.unsigned_abs();
            ca.balance = ca.balance.saturating_sub(loss.min(ca.balance));
        }
    }

    fn remove_cross_pos_id(&mut self, trader: Address, pos_id: u64) {
        if let Some(ca) = self.cross_accounts.get_mut(&trader) {
            ca.pos_ids.retain(|&id| id != pos_id);
        }
    }

    fn assert_owner(&self, sender: Address, pos_id: u64) -> Result<(), PerpError> {
        let p = self.positions.get(&pos_id).ok_or(PerpError::PositionNotFound(pos_id))?;
        if p.trader != sender { return Err(PerpError::NotPositionOwner(pos_id)); }
        if p.closed  { return Err(PerpError::AlreadyClosed(pos_id)); }
        Ok(())
    }

    pub fn next_position_id(&self) -> u64 { self.next_pos_id }
}

// ─── PnL formula ─────────────────────────────────────────────────────────────

/// Compute signed PnL for a position given entry and exit price.
/// Matches _pnlFor() in ZbxPerpetuals.sol.
///   LONG:  pnl = (exit − entry) × size / entry
///   SHORT: pnl = (entry − exit) × size / entry
pub fn pnl_for(is_long: bool, entry: u128, exit: u128, size: u128) -> i128 {
    if entry == 0 || size == 0 { return 0; }
    let signed_price_diff: i128 = if is_long {
        (exit as i128).saturating_sub(entry as i128)
    } else {
        (entry as i128).saturating_sub(exit as i128)
    };
    signed_price_diff.saturating_mul(size as i128)
        .checked_div(entry as i128)
        .unwrap_or(0)
}

/// Keeper bounty for triggering a SL/TP order (0.05% of position collateral).
pub fn keeper_bounty_for(collateral: u128) -> u128 {
    (collateral * KEEPER_BOUNTY_BPS as u128) / 10_000
}

/// Liquidation bounty for liquidating a position (1% of collateral).
pub fn liquidation_bounty_for(collateral: u128) -> u128 {
    (collateral * LIQUIDATION_BOUNTY_BPS as u128) / 10_000
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::OpenPositionParams;

    fn addr(b: u8) -> Address { Address([b; 20]) }
    const ONE_ETH: u128 = 1_000_000_000_000_000_000;

    fn dummy_market() -> Market {
        Market {
            symbol: "BTC".into(),
            oracle: addr(99),
            active: true,
            max_leverage: 200,
            total_long_oi: 0,
            total_short_oi: 0,
            cumulative_funding: 0,
            last_funding_update: 0,
        }
    }

    fn open_params(is_long: bool, is_cross: bool, collateral: u128, leverage: u64) -> OpenPositionParams {
        OpenPositionParams {
            market_id: 0,
            is_long,
            collateral,
            leverage,
            is_cross,
            sl_price: 0,
            tp_price: 0,
        }
    }

    #[test]
    fn open_isolated_long_and_close_in_profit() {
        let mut store = PositionStore::new();
        let mut market = dummy_market();
        let trader = addr(1);
        let col = 100 * ONE_ETH;
        let entry = 50_000 * ONE_ETH;

        let res = store.open(trader, &open_params(true, false, col, 10), entry, &mut market).unwrap();
        assert!(res.size > 0);
        assert_eq!(market.total_long_oi, res.size);

        let exit = 55_000 * ONE_ETH; // 10% up
        let cr = store.close(trader, res.position_id, exit, &mut market).unwrap();
        assert!(cr.net_pnl > 0);
        assert_eq!(market.total_long_oi, 0);
    }

    #[test]
    fn open_isolated_short_and_close_at_loss() {
        let mut store = PositionStore::new();
        let mut market = dummy_market();
        let trader = addr(2);
        let col = 50 * ONE_ETH;
        let entry = 50_000 * ONE_ETH;

        let res = store.open(trader, &open_params(false, false, col, 5), entry, &mut market).unwrap();
        let exit = 52_000 * ONE_ETH; // price went up — short loses
        let cr = store.close(trader, res.position_id, exit, &mut market).unwrap();
        assert!(cr.net_pnl < 0);
    }

    #[test]
    fn partial_close_50_percent() {
        let mut store = PositionStore::new();
        let mut market = dummy_market();
        let trader = addr(3);
        let col = 100 * ONE_ETH;
        let entry = 50_000 * ONE_ETH;

        let res = store.open(trader, &open_params(true, false, col, 10), entry, &mut market).unwrap();
        let original_size = res.size;
        let exit = 51_000 * ONE_ETH;
        store.partial_close(trader, res.position_id, 5_000, exit, &mut market).unwrap();

        let pos = store.get_position(res.position_id).unwrap();
        assert_eq!(pos.size, original_size / 2);
    }

    #[test]
    fn add_collateral_increases_margin() {
        let mut store = PositionStore::new();
        let mut market = dummy_market();
        let trader = addr(4);
        let col = 100 * ONE_ETH;

        let res = store.open(trader, &open_params(true, false, col, 10), 50_000 * ONE_ETH, &mut market).unwrap();
        let before = store.get_position(res.position_id).unwrap().collateral;
        store.add_collateral(res.position_id, 10 * ONE_ETH).unwrap();
        let after = store.get_position(res.position_id).unwrap().collateral;
        assert_eq!(after, before + 10 * ONE_ETH);
    }

    #[test]
    fn pnl_for_long_profit() {
        let pnl = pnl_for(true, 100, 110, 1000);
        assert_eq!(pnl, 100); // (110-100) * 1000 / 100 = 100
    }

    #[test]
    fn pnl_for_long_loss() {
        let pnl = pnl_for(true, 100, 90, 1000);
        assert_eq!(pnl, -100); // (90-100) * 1000 / 100 = -100
    }

    #[test]
    fn pnl_for_short_profit() {
        let pnl = pnl_for(false, 100, 90, 1000);
        assert_eq!(pnl, 100); // (100-90) * 1000 / 100 = 100
    }

    #[test]
    fn health_drops_to_zero_near_liquidation() {
        let mut store = PositionStore::new();
        let mut market = dummy_market();
        let trader = addr(5);
        let col = 100 * ONE_ETH;
        let entry = 50_000 * ONE_ETH;

        let res = store.open(trader, &open_params(true, false, col, 10), entry, &mut market).unwrap();
        let pos = store.get_position(res.position_id).unwrap();
        // Drop price by ~90% — far below liquidation
        let low_price = entry / 10;
        let h = store.health_bps(res.position_id, low_price, &market);
        assert_eq!(h, 0, "health must be 0 when deeply underwater");
        _ = pos;
    }
}
