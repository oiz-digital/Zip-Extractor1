//! Isolated and cross-margin liquidation engine.
//!
//! Mirrors `liquidate(uint256)` and `liquidateCross(address)` from
//! ZbxPerpetuals.sol — earns the caller a 1% bounty on the liquidated collateral.

use zbx_types::address::Address;
use crate::error::PerpError;
use crate::position::{liquidation_bounty_for, PositionStore};
use crate::types::{LiquidationResult, Market};
use crate::MAINTENANCE_MARGIN_BPS;

// ─── Isolated liquidation ────────────────────────────────────────────────────

/// Liquidate an isolated (non-cross) position that is below maintenance margin.
///
/// ## Bounty
/// Keeper earns `LIQUIDATION_BOUNTY_BPS` (1%) of the position's collateral.
/// Remaining collateral goes to `protocol_fee_balance`.
///
/// Returns `LiquidationResult` — caller handles the token transfer to the keeper.
pub fn liquidate(
    store:   &mut PositionStore,
    keeper:  Address,
    pos_id:  u64,
    mark:    u128,
    market:  &mut Market,
) -> Result<LiquidationResult, PerpError> {
    let p = store.get_position(pos_id).ok_or(PerpError::PositionNotFound(pos_id))?;
    if p.closed    { return Err(PerpError::AlreadyClosed(pos_id)); }
    if p.is_cross  { return Err(PerpError::NotIsolatedPosition); }

    if !is_isolated_liquidatable(p, mark, market) {
        return Err(PerpError::NotLiquidatable(pos_id));
    }

    let collateral = p.collateral;
    let size       = p.size;
    let is_long    = p.is_long;

    // Close position (no PnL payout — all collateral goes to bounty + protocol)
    if is_long {
        market.total_long_oi = market.total_long_oi.saturating_sub(size);
    } else {
        market.total_short_oi = market.total_short_oi.saturating_sub(size);
    }
    let p = store.positions.get_mut(&pos_id).unwrap();
    p.closed = true;

    let bounty       = liquidation_bounty_for(collateral);
    let protocol_fee = if collateral > bounty { collateral - bounty } else { 0 };
    store.protocol_fee_balance = store.protocol_fee_balance.saturating_add(protocol_fee);

    tracing::info!(
        pos_id, ?keeper, collateral, bounty, protocol_fee,
        "isolated position liquidated"
    );

    Ok(LiquidationResult { exit_price: mark, keeper_bounty: bounty, protocol_fee })
}

// ─── Cross liquidation ───────────────────────────────────────────────────────

/// Liquidate an entire cross-margin account when equity < maintenance margin.
///
/// Closes ALL open cross positions for the trader, awards 1% of the account
/// balance as a keeper bounty, and sweeps the remainder to `protocol_fee_balance`.
///
/// Returns a `LiquidationResult` where `keeper_bounty` is drawn from the
/// cross account balance.
pub fn liquidate_cross(
    store:  &mut PositionStore,
    keeper: Address,
    trader: Address,
    markets: &mut dyn FnMut(u64) -> Option<u128>, // closure: market_id → current mark price
) -> Result<LiquidationResult, PerpError> {
    if !is_cross_liquidatable(store, trader) {
        return Err(PerpError::NotLiquidatable(0));
    }

    let pos_ids: Vec<u64> = store.cross_position_ids(trader);
    let mut count = 0u64;

    for pid in &pos_ids {
        let Some(p) = store.positions.get(pid) else { continue; };
        if p.closed { continue; }
        let market_id = p.market_id;
        let mark_opt  = markets(market_id);
        let Some(_mark) = mark_opt else { continue; };

        let p = store.positions.get_mut(pid).unwrap();
        p.closed = true;
        count += 1;
    }

    let ca = store.cross_accounts.entry(trader).or_default();
    let balance = ca.balance;
    let bounty  = liquidation_bounty_for(balance);
    let safe_bounty = if bounty <= balance { bounty } else { 0 };
    let remaining = balance.saturating_sub(safe_bounty);

    ca.balance       = 0;
    ca.initial_margin = 0;
    ca.pos_ids.clear();

    store.protocol_fee_balance = store.protocol_fee_balance.saturating_add(remaining);

    tracing::info!(
        ?trader, ?keeper, count, balance, bounty = safe_bounty,
        "cross account liquidated"
    );

    Ok(LiquidationResult {
        exit_price:    0, // no single exit price for cross
        keeper_bounty: safe_bounty,
        protocol_fee:  remaining,
    })
}

// ─── Predicates ──────────────────────────────────────────────────────────────

/// Is an isolated position liquidatable?
/// A position is liquidatable when its equity ≤ maintenance margin.
pub fn is_isolated_liquidatable(
    p:      &crate::types::Position,
    mark:   u128,
    market: &Market,
) -> bool {
    if p.closed || p.is_cross { return false; }
    use crate::funding::funding_cost_for_position;
    use crate::position::pnl_for;
    let pnl    = pnl_for(p.is_long, p.entry_price, mark, p.size);
    let fund   = funding_cost_for_position(market.cumulative_funding, p.funding_entry_rate, p.size);
    let eq: i128 = (p.collateral as i128).saturating_add(pnl).saturating_sub(fund);
    let maint  = ((p.size * MAINTENANCE_MARGIN_BPS as u128) / 10_000) as i128;
    eq <= maint
}

/// Is a cross account eligible for liquidation?
/// Liquidatable when cross equity ≤ sum of maintenance margins.
pub fn is_cross_liquidatable(store: &PositionStore, trader: Address) -> bool {
    let maint = store.cross_maint_margin(trader);
    let bal   = store.cross_balance(trader);
    (bal as i128) < maint as i128
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::position::PositionStore;
    use crate::types::{Market, OpenPositionParams};
    use zbx_types::address::Address;

    fn addr(b: u8) -> Address { Address([b; 20]) }
    const ONE: u128 = 1_000_000_000_000_000_000;

    fn dummy_market() -> Market {
        Market {
            symbol: "BTC".into(), oracle: addr(0), active: true, max_leverage: 200,
            total_long_oi: 0, total_short_oi: 0, cumulative_funding: 0, last_funding_update: 0,
        }
    }

    fn open_isolated(store: &mut PositionStore, is_long: bool, col: u128, lev: u64, entry: u128) -> (u64, Market) {
        let mut m = dummy_market();
        let p = OpenPositionParams {
            market_id: 0, is_long, collateral: col, leverage: lev,
            is_cross: false, sl_price: 0, tp_price: 0,
        };
        let res = store.open(addr(1), &p, entry, &mut m).unwrap();
        (res.position_id, m)
    }

    #[test]
    fn liquidate_healthy_position_fails() {
        let mut store = PositionStore::new();
        let (pid, mut m) = open_isolated(&mut store, true, 100 * ONE, 10, 50_000 * ONE);
        // Mark still at entry — not liquidatable
        let err = liquidate(&mut store, addr(99), pid, 50_000 * ONE, &mut m).unwrap_err();
        assert_eq!(err, PerpError::NotLiquidatable(pid));
    }

    #[test]
    fn liquidate_underwater_position_succeeds() {
        let mut store = PositionStore::new();
        let (pid, mut m) = open_isolated(&mut store, true, 100 * ONE, 10, 50_000 * ONE);
        // Drop price to near zero — deeply liquidatable
        let res = liquidate(&mut store, addr(99), pid, 100, &mut m).unwrap();
        assert!(res.keeper_bounty > 0);
        let p = store.get_position(pid).unwrap();
        assert!(p.closed);
    }

    #[test]
    fn liquidate_cross_position_directly_rejected() {
        let mut store = PositionStore::new();
        let mut m = dummy_market();
        store.deposit_cross(addr(1), 10_000 * ONE).unwrap();
        let p = OpenPositionParams {
            market_id: 0, is_long: true, collateral: 100 * ONE, leverage: 1,
            is_cross: true, sl_price: 0, tp_price: 0,
        };
        let res = store.open(addr(1), &p, 1_000 * ONE, &mut m).unwrap();
        let err = liquidate(&mut store, addr(99), res.position_id, 1 * ONE, &mut m).unwrap_err();
        assert_eq!(err, PerpError::NotIsolatedPosition);
    }
}
