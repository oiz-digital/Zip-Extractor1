//! SL / TP / Trailing-stop management and keeper trigger logic.
//!
//! Mirrors `setStopLoss`, `setTakeProfit`, `setTrailingStop`,
//! `updateTrailingStop`, `triggerOrder`, `triggerStopLoss`,
//! `triggerTakeProfit` from ZbxPerpetuals.sol.

use zbx_types::address::Address;
use crate::error::PerpError;
use crate::position::{keeper_bounty_for, PositionStore};
use crate::types::{CloseResult, Market};
use crate::MAX_TRAIL_BPS;

// ─── SL / TP setters ─────────────────────────────────────────────────────────

/// Validate and set the stop-loss price on a position.
/// Pass `sl_price = 0` to remove the stop-loss.
pub fn set_stop_loss(
    store:    &mut PositionStore,
    sender:   Address,
    pos_id:   u64,
    sl_price: u128,
    mark:     u128,
) -> Result<(), PerpError> {
    let p = store.get_position(pos_id).ok_or(PerpError::PositionNotFound(pos_id))?;
    if p.trader != sender  { return Err(PerpError::NotPositionOwner(pos_id)); }
    if p.closed             { return Err(PerpError::AlreadyClosed(pos_id)); }
    if sl_price != 0 {
        validate_sl(p.is_long, mark, sl_price)?;
    }
    // SAFETY: we already checked the position exists above.
    let p = store.positions.get_mut(&pos_id).unwrap();
    p.stop_loss = sl_price;
    Ok(())
}

/// Validate and set the take-profit price on a position.
/// Pass `tp_price = 0` to remove the take-profit.
pub fn set_take_profit(
    store:    &mut PositionStore,
    sender:   Address,
    pos_id:   u64,
    tp_price: u128,
    mark:     u128,
) -> Result<(), PerpError> {
    let p = store.get_position(pos_id).ok_or(PerpError::PositionNotFound(pos_id))?;
    if p.trader != sender { return Err(PerpError::NotPositionOwner(pos_id)); }
    if p.closed            { return Err(PerpError::AlreadyClosed(pos_id)); }
    if tp_price != 0 {
        validate_tp(p.is_long, mark, tp_price)?;
    }
    let p = store.positions.get_mut(&pos_id).unwrap();
    p.take_profit = tp_price;
    Ok(())
}

/// Set a trailing stop on a position.
///
/// `trail_bps` = trail width in basis points (1–MAX_TRAIL_BPS = 5000 = 50%).
///
/// Initialises `trail_peak` to current mark price and sets `stop_loss` to
/// the trailing-stop initial level:
///   LONG:  sl = mark × (10000 − trail_bps) / 10000
///   SHORT: sl = mark × (10000 + trail_bps) / 10000
pub fn set_trailing_stop(
    store:     &mut PositionStore,
    sender:    Address,
    pos_id:    u64,
    trail_bps: u64,
    mark:      u128,
) -> Result<(), PerpError> {
    if trail_bps == 0 || trail_bps > MAX_TRAIL_BPS {
        return Err(PerpError::InvalidTrailBps);
    }
    let p = store.get_position(pos_id).ok_or(PerpError::PositionNotFound(pos_id))?;
    if p.trader != sender { return Err(PerpError::NotPositionOwner(pos_id)); }
    if p.closed            { return Err(PerpError::AlreadyClosed(pos_id)); }

    let init_sl = compute_trail_sl(p.is_long, mark, trail_bps);
    let p = store.positions.get_mut(&pos_id).unwrap();
    p.trail_bps  = trail_bps;
    p.trail_peak = mark;
    p.stop_loss  = init_sl;
    Ok(())
}

/// Ratchet the trailing stop when mark price has improved past the peak.
///
/// Caller (keeper or trader) calls this when mark price has moved favourably.
/// Reverts with `TrailNotFavourable` if price has not improved.
///
/// Returns the new stop-loss price.
pub fn update_trailing_stop(
    store:  &mut PositionStore,
    pos_id: u64,
    mark:   u128,
) -> Result<u128, PerpError> {
    let p = store.get_position(pos_id).ok_or(PerpError::PositionNotFound(pos_id))?;
    if p.closed || p.trail_bps == 0 { return Err(PerpError::InvalidBps); }

    let improved = if p.is_long {
        mark > p.trail_peak
    } else {
        mark < p.trail_peak
    };
    if !improved { return Err(PerpError::TrailNotFavourable); }

    let trail_bps = p.trail_bps;
    let new_sl    = compute_trail_sl(p.is_long, mark, trail_bps);
    let p = store.positions.get_mut(&pos_id).unwrap();
    p.trail_peak = mark;
    p.stop_loss  = new_sl;
    Ok(new_sl)
}

// ─── Keeper triggers ─────────────────────────────────────────────────────────

/// Unified trigger — executes whichever of SL or TP is currently hit.
/// Earns `KEEPER_BOUNTY_BPS` (0.05%) of the position's collateral.
///
/// Mirrors `triggerOrder(uint256)` in ZbxPerpetuals.sol.
pub fn trigger_order(
    store:   &mut PositionStore,
    keeper:  Address,
    pos_id:  u64,
    mark:    u128,
    market:  &mut Market,
) -> Result<(CloseResult, u128), PerpError> {
    let p = store.get_position(pos_id).ok_or(PerpError::PositionNotFound(pos_id))?;
    if p.closed { return Err(PerpError::AlreadyClosed(pos_id)); }

    let sl_hit = sl_hit(p, mark);
    let tp_hit = tp_hit(p, mark);
    if !sl_hit && !tp_hit {
        return Err(PerpError::NeitherTriggered(pos_id));
    }

    let bounty = keeper_bounty_for(p.collateral);
    tracing::info!(pos_id, ?keeper, sl_hit, tp_hit, bounty, "order triggered");

    store.protocol_fee_balance = store.protocol_fee_balance.saturating_sub(bounty);
    // The bounty comes from collateral; caller must credit `keeper` with `bounty` wei.
    let cr = store.close_unchecked(pos_id, mark, market)?;
    Ok((cr, bounty))
}

/// Trigger the stop-loss specifically.
pub fn trigger_stop_loss(
    store:  &mut PositionStore,
    keeper: Address,
    pos_id: u64,
    mark:   u128,
    market: &mut Market,
) -> Result<(CloseResult, u128), PerpError> {
    let p = store.get_position(pos_id).ok_or(PerpError::PositionNotFound(pos_id))?;
    if p.closed { return Err(PerpError::AlreadyClosed(pos_id)); }
    if !sl_hit(p, mark) { return Err(PerpError::SLNotTriggered(pos_id)); }

    let bounty = keeper_bounty_for(p.collateral);
    tracing::info!(pos_id, ?keeper, bounty, "stop-loss triggered");
    store.protocol_fee_balance = store.protocol_fee_balance.saturating_sub(bounty);
    let cr = store.close_unchecked(pos_id, mark, market)?;
    Ok((cr, bounty))
}

/// Trigger the take-profit specifically.
pub fn trigger_take_profit(
    store:  &mut PositionStore,
    keeper: Address,
    pos_id: u64,
    mark:   u128,
    market: &mut Market,
) -> Result<(CloseResult, u128), PerpError> {
    let p = store.get_position(pos_id).ok_or(PerpError::PositionNotFound(pos_id))?;
    if p.closed { return Err(PerpError::AlreadyClosed(pos_id)); }
    if !tp_hit(p, mark) { return Err(PerpError::TPNotTriggered(pos_id)); }

    let bounty = keeper_bounty_for(p.collateral);
    tracing::info!(pos_id, ?keeper, bounty, "take-profit triggered");
    store.protocol_fee_balance = store.protocol_fee_balance.saturating_sub(bounty);
    let cr = store.close_unchecked(pos_id, mark, market)?;
    Ok((cr, bounty))
}

// ─── Validation helpers ───────────────────────────────────────────────────────

/// Validate a stop-loss price against the current mark price.
///   LONG:  sl < mark (stop below current price)
///   SHORT: sl > mark (stop above current price)
pub fn validate_sl(is_long: bool, mark: u128, sl: u128) -> Result<(), PerpError> {
    if is_long  && sl >= mark { return Err(PerpError::InvalidStopLoss); }
    if !is_long && sl <= mark { return Err(PerpError::InvalidStopLoss); }
    Ok(())
}

/// Validate a take-profit price against the current mark price.
///   LONG:  tp > mark (profit above current price)
///   SHORT: tp < mark (profit below current price)
pub fn validate_tp(is_long: bool, mark: u128, tp: u128) -> Result<(), PerpError> {
    if is_long  && tp <= mark { return Err(PerpError::InvalidTakeProfit); }
    if !is_long && tp >= mark { return Err(PerpError::InvalidTakeProfit); }
    Ok(())
}

// ─── Trigger predicates ───────────────────────────────────────────────────────

pub fn sl_hit(p: &crate::types::Position, mark: u128) -> bool {
    p.stop_loss > 0
        && ((p.is_long && mark <= p.stop_loss) || (!p.is_long && mark >= p.stop_loss))
}

pub fn tp_hit(p: &crate::types::Position, mark: u128) -> bool {
    p.take_profit > 0
        && ((p.is_long && mark >= p.take_profit) || (!p.is_long && mark <= p.take_profit))
}

// ─── Trailing stop computation ────────────────────────────────────────────────

fn compute_trail_sl(is_long: bool, mark: u128, trail_bps: u64) -> u128 {
    if is_long {
        mark * (10_000 - trail_bps as u128) / 10_000
    } else {
        mark * (10_000 + trail_bps as u128) / 10_000
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::OpenPositionParams;
    use zbx_types::address::Address;
    use crate::types::Market;

    fn addr(b: u8) -> Address { Address([b; 20]) }
    const ONE: u128 = 1_000_000_000_000_000_000;

    fn dummy_market() -> Market {
        Market {
            symbol: "X".into(), oracle: addr(99), active: true, max_leverage: 200,
            total_long_oi: 0, total_short_oi: 0, cumulative_funding: 0,
            last_funding_update: 0,
        }
    }

    fn open_long(store: &mut PositionStore, entry: u128) -> u64 {
        let mut m = dummy_market();
        let p = OpenPositionParams {
            market_id: 0, is_long: true, collateral: 100 * ONE,
            leverage: 10, is_cross: false, sl_price: 0, tp_price: 0,
        };
        store.open(addr(1), &p, entry, &mut m).unwrap().position_id
    }

    #[test]
    fn validate_sl_long_requires_sl_below_mark() {
        assert!(validate_sl(true, 1000, 900).is_ok());
        assert!(validate_sl(true, 1000, 1000).is_err());
        assert!(validate_sl(true, 1000, 1100).is_err());
    }

    #[test]
    fn validate_sl_short_requires_sl_above_mark() {
        assert!(validate_sl(false, 1000, 1100).is_ok());
        assert!(validate_sl(false, 1000, 1000).is_err());
        assert!(validate_sl(false, 1000, 900).is_err());
    }

    #[test]
    fn validate_tp_long_requires_tp_above_mark() {
        assert!(validate_tp(true, 1000, 1100).is_ok());
        assert!(validate_tp(true, 1000, 1000).is_err());
        assert!(validate_tp(true, 1000, 900).is_err());
    }

    #[test]
    fn set_trailing_stop_sets_correct_sl() {
        let mut store = PositionStore::new();
        let pid = open_long(&mut store, 50_000 * ONE);
        set_trailing_stop(&mut store, addr(1), pid, 200, 50_000 * ONE).unwrap();
        let p = store.get_position(pid).unwrap();
        assert_eq!(p.trail_bps, 200);
        let expected_sl = 50_000 * ONE * (10_000 - 200) / 10_000;
        assert_eq!(p.stop_loss, expected_sl);
    }

    #[test]
    fn update_trailing_stop_ratchets_on_new_high() {
        let mut store = PositionStore::new();
        let pid = open_long(&mut store, 50_000 * ONE);
        set_trailing_stop(&mut store, addr(1), pid, 200, 50_000 * ONE).unwrap();
        let new_sl = update_trailing_stop(&mut store, pid, 52_000 * ONE).unwrap();
        let expected = 52_000 * ONE * (10_000 - 200) / 10_000;
        assert_eq!(new_sl, expected);
    }

    #[test]
    fn update_trailing_stop_rejects_unfavourable() {
        let mut store = PositionStore::new();
        let pid = open_long(&mut store, 50_000 * ONE);
        set_trailing_stop(&mut store, addr(1), pid, 200, 50_000 * ONE).unwrap();
        let err = update_trailing_stop(&mut store, pid, 49_000 * ONE).unwrap_err();
        assert_eq!(err, PerpError::TrailNotFavourable);
    }

    #[test]
    fn sl_hit_returns_true_when_price_at_or_below_sl() {
        let p = crate::types::Position {
            trader: addr(1), market_id: 0, is_long: true, is_cross: false,
            collateral: 100, size: 1000, entry_price: 1000, funding_entry_rate: 0,
            stop_loss: 900, take_profit: 0, trail_bps: 0, trail_peak: 1000,
            closed: false, initial_margin: 100,
        };
        assert!(sl_hit(&p, 899));
        assert!(sl_hit(&p, 900));
        assert!(!sl_hit(&p, 901));
    }
}
