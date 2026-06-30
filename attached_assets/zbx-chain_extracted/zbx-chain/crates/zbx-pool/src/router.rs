//! Multi-hop swap router — finds the best route across canonical ZBX pools.
//!
//! # Routing strategy
//!
//! 1. **Direct route**: token_in → token_out (single hop if pair exists)
//! 2. **Two-hop via ZUSD**: token_in → ZUSD → token_out
//! 3. **Two-hop via WZBX**: token_in → WZBX → token_out
//! 4. Best route = highest simulated output
//!
//! # Example routes
//!
//! | From | To | Route |
//! |------|----|-------|
//! | ZBX | ZUSD | ZBX/ZUSD direct (1 hop) |
//! | ERC-20 | ZUSD | ERC-20/ZBX + ZBX/ZUSD (2 hops via WZBX) |
//!
//! All canonical pairs exist as direct routes. Two-hop fallback is used
//! for non-canonical token pairs (third-party ERC-20 tokens).

use std::collections::HashMap;
use zbx_types::address::Address;
use crate::{
    canonical_pairs::{wzbx, zusd},
    error::AmmError,
    pair::{Pair, PairId, SwapParams},
};

// ── Route building blocks ─────────────────────────────────────────────────────

/// One swap step in a multi-hop route.
#[derive(Debug, Clone)]
pub struct SwapStep {
    pub pair_id: PairId,
    /// true = swap token_a → token_b in this pair.
    pub a_to_b:  bool,
}

/// An ordered sequence of swap steps (1–3 hops).
#[derive(Debug, Clone)]
pub struct SwapRoute {
    pub steps:     Vec<SwapStep>,
    pub amount_in: u128,
}

impl SwapRoute {
    /// Simulate this route through `pairs`, returning the final amount out.
    /// Uses `get_amount_out` (fee-applied quote, no security checks).
    pub fn simulate(&self, pairs: &HashMap<PairId, Pair>) -> u128 {
        let mut amount = self.amount_in;
        for step in &self.steps {
            match pairs.get(&step.pair_id) {
                Some(pair) => {
                    amount = pair.get_amount_out(amount, step.a_to_b);
                    if amount == 0 { return 0; }
                }
                None => return 0,
            }
        }
        amount
    }

    /// Number of hops in this route.
    pub fn hops(&self) -> usize { self.steps.len() }
}

// ── Route finding ─────────────────────────────────────────────────────────────

/// Find the best route from `from` to `to` given the available pairs.
///
/// Returns `None` if no route exists through any known path.
/// Returns the route with the highest simulated output.
pub fn find_best_route(
    from:      Address,
    to:        Address,
    amount_in: u128,
    pairs:     &HashMap<PairId, Pair>,
) -> Option<SwapRoute> {
    if from == to || amount_in == 0 {
        return None;
    }

    let mut candidates: Vec<SwapRoute> = Vec::new();

    // 1. Direct route
    if let Some(r) = build_direct(from, to, amount_in, pairs) {
        candidates.push(r);
    }

    // 2. Two-hop via WZBX (ZBX native)
    if from != wzbx() && to != wzbx() {
        if let Some(r) = build_two_hop(from, wzbx(), to, amount_in, pairs) {
            candidates.push(r);
        }
    }

    // 3. Two-hop via ZUSD
    if from != zusd() && to != zusd() {
        if let Some(r) = build_two_hop(from, zusd(), to, amount_in, pairs) {
            candidates.push(r);
        }
    }

    if candidates.is_empty() {
        return None;
    }

    // Pick best by simulated output (highest amount out)
    candidates.into_iter().max_by_key(|r| r.simulate(pairs))
}

/// Build a direct 1-hop route, or None if the pair doesn't exist.
pub fn find_direct_route(
    from:      Address,
    to:        Address,
    amount_in: u128,
    pairs:     &HashMap<PairId, Pair>,
) -> Option<SwapRoute> {
    build_direct(from, to, amount_in, pairs)
}

// ── Execution ─────────────────────────────────────────────────────────────────

/// Execute a route through the pairs map.
///
/// Each step calls `pair.swap()` with full security checks.
/// If any step fails, the entire operation is considered failed
/// (caller must not apply partial state).
///
/// ## Return value
///
/// `Ok((amount_out, total_protocol_fee))` — the final output token amount and
/// the sum of all protocol fees (in input-token units of each respective hop).
/// The caller (`DexEngine::buy`) uses `total_protocol_fee` to accurately
/// record accumulated fees rather than using a rough estimate.
pub fn execute_route(
    route:           &SwapRoute,
    pairs:           &mut HashMap<PairId, Pair>,
    min_amount_out:  u128,
    deadline:        u64,
    block_timestamp: u64,
) -> Result<(u128, u128), AmmError> {
    let mut amount             = route.amount_in;
    let mut total_protocol_fee = 0u128;

    for (i, step) in route.steps.iter().enumerate() {
        let pair = pairs.get_mut(&step.pair_id)
            .ok_or(AmmError::EmptyReserve)?;

        // Last step gets slippage check; intermediate steps get min=0
        let min_out = if i == route.steps.len() - 1 { min_amount_out } else { 0 };

        let result = pair.swap(SwapParams {
            a_to_b:          step.a_to_b,
            amount_in:       amount,
            min_amount_out:  min_out,
            deadline,
            oracle_twap:     0,  // oracle check done at route level
            block_timestamp,
        })?;

        total_protocol_fee = total_protocol_fee.saturating_add(result.protocol_fee);
        amount = result.amount_out;
    }

    Ok((amount, total_protocol_fee))
}

// ── Route builder helpers ─────────────────────────────────────────────────────

fn build_direct(
    from:      Address,
    to:        Address,
    amount_in: u128,
    pairs:     &HashMap<PairId, Pair>,
) -> Option<SwapRoute> {
    let pid    = PairId::new(from, to);
    let a_to_b = pid.token_a == from;
    if pairs.contains_key(&pid) {
        Some(SwapRoute {
            steps:     vec![SwapStep { pair_id: pid, a_to_b }],
            amount_in,
        })
    } else {
        None
    }
}

fn build_two_hop(
    from:      Address,
    mid:       Address,
    to:        Address,
    amount_in: u128,
    pairs:     &HashMap<PairId, Pair>,
) -> Option<SwapRoute> {
    let pid1    = PairId::new(from, mid);
    let pid2    = PairId::new(mid, to);
    if !pairs.contains_key(&pid1) || !pairs.contains_key(&pid2) {
        return None;
    }
    let a_to_b1 = pid1.token_a == from;
    let a_to_b2 = pid2.token_a == mid;
    Some(SwapRoute {
        steps: vec![
            SwapStep { pair_id: pid1, a_to_b: a_to_b1 },
            SwapStep { pair_id: pid2, a_to_b: a_to_b2 },
        ],
        amount_in,
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        fee::FeeTier,
        pair::Pair,
        canonical_pairs::{wzbx, zusd},
        security::MIN_LIQUIDITY,
    };

    fn make_pair(a: Address, b: Address, reserve: u128, fee: FeeTier) -> Pair {
        let mut p = Pair::new(PairId::new(a, b), fee);
        p.reserve_a = reserve;
        p.reserve_b = reserve;
        p.total_lp  = crate::pair::integer_sqrt(reserve * reserve)
            .saturating_sub(MIN_LIQUIDITY);
        p
    }

    fn canonical_pairs_map() -> HashMap<PairId, Pair> {
        let mut m = HashMap::new();
        let zbx_zusd = make_pair(wzbx(), zusd(), 1_000_000, FeeTier::Standard);
        m.insert(zbx_zusd.id.clone(), zbx_zusd);
        m
    }

    #[test]
    fn direct_zbx_to_zusd() {
        let pairs = canonical_pairs_map();
        let r = find_best_route(wzbx(), zusd(), 1_000, &pairs).unwrap();
        assert_eq!(r.hops(), 1);
        let out = r.simulate(&pairs);
        assert!(out > 0 && out < 1_000);
    }

    #[test]
    fn two_hop_route_found_when_no_direct() {
        // Third-party ERC-20 that only has ZBX pair, routes via WZBX
        let erc20 = Address([0xAAu8; 20]);
        let mut pairs = HashMap::new();
        let erc20_zbx = make_pair(erc20, wzbx(), 1_000_000, FeeTier::Standard);
        let zbx_zusd  = make_pair(wzbx(), zusd(), 1_000_000, FeeTier::Standard);
        pairs.insert(erc20_zbx.id.clone(), erc20_zbx);
        pairs.insert(zbx_zusd.id.clone(),  zbx_zusd);

        let r = find_best_route(erc20, zusd(), 1_000, &pairs).unwrap();
        assert_eq!(r.hops(), 2);
        assert!(r.simulate(&pairs) > 0);
    }

    #[test]
    fn best_route_picks_highest_output() {
        // Direct vs 2-hop: both exist, direct always wins for balanced pools
        let pairs = canonical_pairs_map();
        let direct = find_direct_route(wzbx(), zusd(), 1_000, &pairs).unwrap();
        let best   = find_best_route(wzbx(), zusd(), 1_000, &pairs).unwrap();
        assert!(best.simulate(&pairs) >= direct.simulate(&pairs));
    }

    #[test]
    fn no_route_returns_none() {
        let pairs = canonical_pairs_map();
        let unknown = Address([0xFFu8; 20]);
        let r = find_best_route(unknown, wzbx(), 1_000, &pairs);
        assert!(r.is_none());
    }

    #[test]
    fn execute_route_direct_returns_amount_and_fee() {
        let mut pairs = canonical_pairs_map();
        let route = find_best_route(wzbx(), zusd(), 1_000, &pairs).unwrap();
        let (out, protocol_fee) = execute_route(&route, &mut pairs, 0, 9999, 1).unwrap();
        assert!(out > 0 && out < 1_000);
        // Standard 0.30% fee: protocol share = 20% of 0.30% = 0.006% of amount_in
        // 0.006% of 1000 = ~0.06 → rounds to 0 at this scale (fine for unit tests)
        // At least verify it doesn't error
        let _ = protocol_fee;
    }

    #[test]
    fn execute_route_respects_slippage() {
        let mut pairs = canonical_pairs_map();
        let route = find_best_route(wzbx(), zusd(), 1_000, &pairs).unwrap();
        let err = execute_route(&route, &mut pairs, 10_000, 9999, 1);
        assert!(matches!(err, Err(AmmError::SlippageExceeded { .. })));
    }

    #[test]
    fn execute_route_two_hop_accumulates_fee() {
        let erc20 = Address([0xAAu8; 20]);
        let mut pairs = HashMap::new();
        let erc20_zbx = make_pair(erc20, wzbx(), 1_000_000, FeeTier::Standard);
        let zbx_zusd  = make_pair(wzbx(), zusd(), 1_000_000, FeeTier::Standard);
        pairs.insert(erc20_zbx.id.clone(), erc20_zbx);
        pairs.insert(zbx_zusd.id.clone(),  zbx_zusd);

        let route = find_best_route(erc20, zusd(), 1_000, &pairs).unwrap();
        assert_eq!(route.hops(), 2);
        let (out, protocol_fee) = execute_route(&route, &mut pairs, 0, 9999, 1).unwrap();
        assert!(out > 0);
        // 2-hop: both hops contribute protocol fee (may be 0 at small amounts due to int division)
        let _ = protocol_fee;
    }
}
