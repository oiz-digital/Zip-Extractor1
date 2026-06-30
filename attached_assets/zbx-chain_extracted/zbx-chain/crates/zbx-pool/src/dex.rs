//! ZBX DEX Engine — top-level buy/sell coordinator.
//!
//! Combines PoolFactory, TokenFactory, AllowanceRegistry, Router and the
//! fee system into a single entry point that callers (the EVM execution layer,
//! RPC handlers, or tests) interact with.
//!
//! ## Buy / Sell flow
//!
//! ```text
//!                     ┌─────────────────────────────────────────────┐
//!                     │               DexEngine                     │
//!                     │                                             │
//!  buy(ZBX → TOKEN)   │  1. Look up best route (Router)             │
//!  ────────────────►  │  2. Check allowance (AllowanceRegistry)     │
//!                     │  3. Deduct input balance from buyer          │
//!                     │  4. Execute route (PoolFactory pairs)        │
//!                     │  5. Credit output balance to buyer           │
//!                     │  6. Collect protocol fee                     │
//!                     │  7. Emit SwapEvent                           │
//!                     └─────────────────────────────────────────────┘
//! ```
//!
//! ## Supported operations
//!
//! | Method | Description |
//! |--------|-------------|
//! | `buy` | Buy `token_out` using `token_in` (exact input swap) |
//! | `sell` | Sell `token_in` for `token_out` (alias for `buy`) |
//! | `add_liquidity` | Deposit token pair, receive LP tokens |
//! | `remove_liquidity` | Burn LP tokens, receive token pair |
//! | `create_pool` | Deploy a new pair pool (pays pool creation fee) |
//! | `create_token` | Deploy a new ERC-20 token (pays token creation fee) |
//! | `approve` | ERC-20 approve for this DEX as spender |
//! | `transfer` | Direct token transfer |
//! | `transfer_from` | Spend approved allowance |
//! | `quote` | Simulation only — returns estimated output amount |
//! | `estimate_fee` | Returns FeeEstimate for any operation |

use std::collections::HashMap;
use zbx_types::address::Address;
use serde::{Deserialize, Serialize};

use crate::{
    approval::{AllowanceRegistry, ApprovalError},
    error::AmmError,
    factory::{PoolFactory, PoolCreatedEvent},
    fee::FeeTier,
    lp_token::LpRegistry,
    pair::{
        AddLiquidityParams, AddLiquidityResult,
        RemoveLiquidityParams, RemoveLiquidityResult,
        PairId, SwapParams,
    },
    registry::{DexOperation, FeeEstimate, FeeRegistry},
    router::{find_best_route, execute_route},
    token_factory::{CreateTokenParams, TokenFactory, TokenFactoryError, TokenRecord},
};

// ── Error ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum DexError {
    Amm(AmmError),
    Approval(ApprovalError),
    Token(TokenFactoryError),
    NoRoute,
    InsufficientInputBalance { have: u128, need: u128 },
    LpTransferFailed(String),
}

impl std::fmt::Display for DexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Amm(e)   => write!(f, "AMM error: {e}"),
            Self::Approval(e) => write!(f, "Approval error: {e}"),
            Self::Token(e) => write!(f, "Token error: {e}"),
            Self::NoRoute  => write!(f, "no route found for this token pair"),
            Self::InsufficientInputBalance { have, need } =>
                write!(f, "insufficient input balance: have {have}, need {need}"),
            Self::LpTransferFailed(s) => write!(f, "LP transfer failed: {s}"),
        }
    }
}
impl std::error::Error for DexError {}

impl From<AmmError>         for DexError { fn from(e: AmmError)         -> Self { Self::Amm(e) } }
impl From<ApprovalError>    for DexError { fn from(e: ApprovalError)    -> Self { Self::Approval(e) } }
impl From<TokenFactoryError> for DexError { fn from(e: TokenFactoryError) -> Self { Self::Token(e) } }

// ── Event types ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwapEvent {
    pub trader:     Address,
    pub token_in:   Address,
    pub token_out:  Address,
    pub amount_in:  u128,
    pub amount_out: u128,
    pub hops:       usize,
    pub protocol_fee_collected: u128,
    pub block_number: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiquidityEvent {
    pub provider:    Address,
    pub pair_id:     PairId,
    pub lp_minted:   u128,
    pub used_a:      u128,
    pub used_b:      u128,
    pub block_number: u64,
}

// ── Quote result ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuoteResult {
    pub amount_in:   u128,
    pub amount_out:  u128,
    pub hops:        usize,
    pub price_impact_bps: u32,
}

// ── DexEngine ─────────────────────────────────────────────────────────────────

/// Top-level ZBX DEX coordinator.
///
/// Owns the pool registry, token registry, allowance registry and LP registry.
/// All DEX operations go through this struct.
pub struct DexEngine {
    /// All liquidity pools (via factory).
    pub pool_factory:   PoolFactory,
    /// All user-created tokens.
    pub token_factory:  TokenFactory,
    /// ERC-20 allowances and balances.
    pub allowances:     AllowanceRegistry,
    /// LP token balances per pair per account.
    pub lp_registry:    LpRegistry,
    /// Swap events log.
    pub swap_events:    Vec<SwapEvent>,
    /// Pool creation events log.
    pub pool_events:    Vec<PoolCreatedEvent>,
    /// Accumulated protocol swap fees per token.
    protocol_fees:      HashMap<Address, u128>,
}

impl DexEngine {
    pub fn new() -> Self {
        DexEngine {
            pool_factory:  PoolFactory::new(),
            token_factory: TokenFactory::new(),
            allowances:    AllowanceRegistry::new(),
            lp_registry:   LpRegistry::new(),
            swap_events:   Vec::new(),
            pool_events:   Vec::new(),
            protocol_fees: HashMap::new(),
        }
    }

    // ── Pool creation ─────────────────────────────────────────────────────────

    /// Create a new liquidity pool for a token pair.
    /// The `creator` pays `POOL_CREATION_FEE_WEI` ZBX.
    pub fn create_pool(
        &mut self,
        token_a:             Address,
        token_b:             Address,
        fee_tier:            FeeTier,
        creator:             Address,
        creator_zbx_balance: u128,
        block_number:        u64,
    ) -> Result<Address, DexError> {
        let (pool_addr, _fee) = self.pool_factory.create_pool(
            token_a, token_b, fee_tier, creator, creator_zbx_balance, block_number,
        )?;

        // Mirror events list
        if let Some(evt) = self.pool_factory.events.last().cloned() {
            self.pool_events.push(evt);
        }

        Ok(pool_addr)
    }

    // ── Token creation ────────────────────────────────────────────────────────

    /// Deploy a new ERC-20 token and mint total supply to the creator.
    pub fn create_token(
        &mut self,
        params: CreateTokenParams,
    ) -> Result<Address, DexError> {
        let supply = params.total_supply;
        let creator = params.creator;
        let token_addr = self.token_factory.create_token(params)?;
        // Credit initial supply to creator's balance in AllowanceRegistry
        self.allowances.add_balance(token_addr, creator, supply);
        Ok(token_addr)
    }

    // ── Buy / Sell ────────────────────────────────────────────────────────────

    /// Buy `token_out` by spending exactly `amount_in` of `token_in`.
    ///
    /// - Finds the best route (1 or 2 hops).
    /// - Deducts `amount_in` from `trader`'s `token_in` balance.
    /// - Credits `amount_out` to `trader`'s `token_out` balance.
    /// - Collects protocol fee from the swap results.
    ///
    /// `sell()` is an alias — the DEX is symmetric.
    pub fn buy(
        &mut self,
        trader:          Address,
        token_in:        Address,
        token_out:       Address,
        amount_in:       u128,
        min_amount_out:  u128,
        deadline:        u64,
        block_timestamp: u64,
        block_number:    u64,
    ) -> Result<SwapEvent, DexError> {
        // 1. Check trader's input balance
        let balance = self.allowances.balance_of(&token_in, &trader);
        if balance < amount_in {
            return Err(DexError::InsufficientInputBalance {
                have: balance,
                need: amount_in,
            });
        }

        // 2. Find best route through existing pools
        let route = find_best_route(
            token_in,
            token_out,
            amount_in,
            &self.pool_factory.pairs,
        ).ok_or(DexError::NoRoute)?;

        let hops = route.hops();

        // 3. Execute route (mutates pool reserves).
        //
        // execute_route now returns (amount_out, total_protocol_fee).
        // Using the exact protocol fee from each SwapResult.protocol_fee is
        // accurate; the previous code used `amount_in / 500` (~0.20% flat),
        // which diverged from the actual split_fee() values for non-Standard
        // tiers (Lowest = 5 bps, High = 100 bps).
        let (amount_out, total_protocol_fee) = execute_route(
            &route,
            &mut self.pool_factory.pairs,
            min_amount_out,
            deadline,
            block_timestamp,
        )?;

        // 4. Deduct input from trader
        self.allowances.sub_balance(token_in, trader, amount_in)
            .map_err(ApprovalError::from)
            .map_err(DexError::from)?;

        // 5. Credit output to trader
        self.allowances.add_balance(token_out, trader, amount_out);

        // 6. Collect actual protocol fees (from SwapResult.protocol_fee sums).
        *self.protocol_fees.entry(token_in).or_insert(0) += total_protocol_fee;

        // 7. Record volume on all touched pairs
        for step in &route.steps {
            self.pool_factory.record_volume(&step.pair_id, amount_in);
        }

        // 8. Emit swap event
        let event = SwapEvent {
            trader,
            token_in,
            token_out,
            amount_in,
            amount_out,
            hops,
            protocol_fee_collected: total_protocol_fee,
            block_number,
        };
        self.swap_events.push(event.clone());

        Ok(event)
    }

    /// Alias for `buy`. On a DEX, sell(A→B) == buy(B with A).
    pub fn sell(
        &mut self,
        trader:          Address,
        token_in:        Address,
        token_out:       Address,
        amount_in:       u128,
        min_amount_out:  u128,
        deadline:        u64,
        block_timestamp: u64,
        block_number:    u64,
    ) -> Result<SwapEvent, DexError> {
        self.buy(
            trader, token_in, token_out, amount_in,
            min_amount_out, deadline, block_timestamp, block_number,
        )
    }

    // ── Liquidity ─────────────────────────────────────────────────────────────

    /// Add liquidity to a pool. Deducts both tokens from provider, mints LP tokens.
    pub fn add_liquidity(
        &mut self,
        provider:        Address,
        token_a:         Address,
        token_b:         Address,
        amount_a:        u128,
        amount_b:        u128,
        min_lp_out:      u128,
        deadline:        u64,
        block_timestamp: u64,
        block_number:    u64,
    ) -> Result<(AddLiquidityResult, LiquidityEvent), DexError> {
        let pair_id = PairId::new(token_a, token_b);

        // Check balances
        let bal_a = self.allowances.balance_of(&pair_id.token_a, &provider);
        let bal_b = self.allowances.balance_of(&pair_id.token_b, &provider);
        if bal_a < amount_a {
            return Err(DexError::InsufficientInputBalance { have: bal_a, need: amount_a });
        }
        if bal_b < amount_b {
            return Err(DexError::InsufficientInputBalance { have: bal_b, need: amount_b });
        }

        // Execute add_liquidity on the pool
        let pair = self.pool_factory.get_pair_mut(&pair_id)
            .ok_or(AmmError::PairNotFound)?;
        let result = pair.add_liquidity(AddLiquidityParams {
            amount_a,
            amount_b,
            min_lp_out,
            deadline,
            block_timestamp,
        })?;

        // Deduct tokens from provider
        self.allowances.sub_balance(pair_id.token_a, provider, result.used_a)
            .map_err(|e| DexError::Approval(e))?;
        self.allowances.sub_balance(pair_id.token_b, provider, result.used_b)
            .map_err(|e| DexError::Approval(e))?;

        // Mint LP tokens
        self.lp_registry.mint(&pair_id, provider, result.lp_minted);

        let event = LiquidityEvent {
            provider,
            pair_id,
            lp_minted: result.lp_minted,
            used_a:    result.used_a,
            used_b:    result.used_b,
            block_number,
        };

        Ok((result, event))
    }

    /// Remove liquidity from a pool. Burns LP tokens, credits both tokens to provider.
    pub fn remove_liquidity(
        &mut self,
        provider:        Address,
        token_a:         Address,
        token_b:         Address,
        lp_amount:       u128,
        min_a_out:       u128,
        min_b_out:       u128,
        deadline:        u64,
        block_timestamp: u64,
    ) -> Result<RemoveLiquidityResult, DexError> {
        let pair_id = PairId::new(token_a, token_b);

        // Check LP balance
        let lp_bal = self.lp_registry.balance(&pair_id, &provider);
        if lp_bal < lp_amount {
            return Err(DexError::Amm(AmmError::InsufficientLpBalance));
        }

        let pair = self.pool_factory.get_pair_mut(&pair_id)
            .ok_or(AmmError::PairNotFound)?;
        let result = pair.remove_liquidity(RemoveLiquidityParams {
            lp_amount,
            min_a_out,
            min_b_out,
            deadline,
            block_timestamp,
        })?;

        // Burn LP tokens
        self.lp_registry.burn(&pair_id, provider, lp_amount)
            .map_err(|e| DexError::LpTransferFailed(e.to_string()))?;

        // Credit tokens back to provider
        self.allowances.add_balance(pair_id.token_a, provider, result.amount_a);
        self.allowances.add_balance(pair_id.token_b, provider, result.amount_b);

        Ok(result)
    }

    // ── ERC-20 ─────────────────────────────────────────────────────────────────

    /// ERC-20 approve: grant `spender` permission to spend `token` from `owner`.
    pub fn approve(
        &mut self,
        token:           Address,
        owner:           Address,
        spender:         Address,
        amount:          u128,
        expire_at_block: u64,
    ) -> Result<(), DexError> {
        self.allowances.approve(token, owner, spender, amount, expire_at_block)
            .map_err(DexError::from)
    }

    /// ERC-20 transferFrom: move tokens on behalf of owner using approved allowance.
    pub fn transfer_from(
        &mut self,
        token:         Address,
        owner:         Address,
        to:            Address,
        spender:       Address,
        amount:        u128,
        current_block: u64,
    ) -> Result<(), DexError> {
        self.allowances.transfer_from(token, owner, to, spender, amount, current_block)
            .map_err(DexError::from)
    }

    /// Direct transfer (caller must own the tokens).
    pub fn transfer(
        &mut self,
        token:  Address,
        owner:  Address,
        to:     Address,
        amount: u128,
    ) -> Result<(), DexError> {
        self.allowances.transfer(token, owner, to, amount)
            .map_err(DexError::from)
    }

    // ── Quote (simulation only) ───────────────────────────────────────────────

    /// Simulate a swap and return the expected output amount. Does NOT modify state.
    pub fn quote(
        &self,
        token_in:  Address,
        token_out: Address,
        amount_in: u128,
    ) -> Option<QuoteResult> {
        let route = find_best_route(token_in, token_out, amount_in, &self.pool_factory.pairs)?;
        let amount_out = route.simulate(&self.pool_factory.pairs);
        if amount_out == 0 { return None; }

        let hops = route.hops();

        // Accurate multi-hop price impact using compounded per-hop impacts.
        //
        // For each hop the fraction of reserves NOT impacted is:
        //   remaining_i = reserve_in / (reserve_in + amount_in_i)
        //
        // These compound multiplicatively, so total remaining = Π remaining_i.
        // price_impact_bps = 10_000 − cumulative_remaining
        //
        // This replaces the previous single-hop-only estimate which only
        // looked at the first step's reserve and therefore under-estimated
        // price impact on 2-hop routes.
        let mut cumulative_remaining = 10_000u128; // 100.00% in basis points
        let mut sim_amount = route.amount_in;
        for step in &route.steps {
            if let Some(pair) = self.pool_factory.pairs.get(&step.pair_id) {
                let r_in = if step.a_to_b { pair.reserve_a } else { pair.reserve_b };
                let denom = r_in.saturating_add(sim_amount).max(1);
                // remaining fraction (in 10000 units)
                let remaining_bps = r_in.saturating_mul(10_000) / denom;
                cumulative_remaining = cumulative_remaining
                    .saturating_mul(remaining_bps) / 10_000;
                // Advance simulated amount for next hop (fee-applied)
                sim_amount = pair.get_amount_out(sim_amount, step.a_to_b);
                if sim_amount == 0 { break; }
            }
        }
        let price_impact_bps = 10_000u32.saturating_sub(cumulative_remaining as u32);

        Some(QuoteResult { amount_in, amount_out, hops, price_impact_bps })
    }

    // ── Fee estimation ────────────────────────────────────────────────────────

    /// Return a full cost breakdown for any DEX operation.
    pub fn estimate_fee(
        &self,
        op:             DexOperation,
        gas_price_gwei: u64,
        amount:         u128,
    ) -> FeeEstimate {
        self.pool_factory
            .fee_registry()
            .estimate_total_cost(op, gas_price_gwei, amount)
    }

    // ── Queries ───────────────────────────────────────────────────────────────

    pub fn balance_of(&self, token: &Address, owner: &Address) -> u128 {
        self.allowances.balance_of(token, owner)
    }

    pub fn lp_balance(&self, token_a: Address, token_b: Address, owner: &Address) -> u128 {
        let pair_id = PairId::new(token_a, token_b);
        self.lp_registry.balance(&pair_id, owner)
    }

    pub fn get_token(&self, addr: &Address) -> Option<&TokenRecord> {
        self.token_factory.get_token(addr)
    }

    pub fn pool_count(&self) -> usize {
        self.pool_factory.pool_count()
    }

    pub fn token_count(&self) -> usize {
        self.token_factory.token_count()
    }

    pub fn protocol_fees_collected(&self, token: &Address) -> u128 {
        self.protocol_fees.get(token).copied().unwrap_or(0)
    }
}

impl Default for DexEngine { fn default() -> Self { Self::new() } }

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        pair::{integer_sqrt, Pair, PairId},
        security::MIN_LIQUIDITY,
        fee::FeeTier,
        registry::FeeRegistry,
        token_factory::CreateTokenParams,
    };

    fn addr(n: u8) -> Address { Address([n; 20]) }

    fn zbx()  -> Address { addr(1) }
    fn zusd() -> Address { addr(2) }

    /// Bootstrap a DexEngine with one ZBX/ZUSD pool and funded balances.
    fn bootstrap(dex: &mut DexEngine, reserve: u128) {
        let pool_fee = dex.pool_factory.fee_registry().pool_creation_fee();

        // Create pool
        dex.create_pool(zbx(), zusd(), FeeTier::Standard, addr(99), pool_fee * 2, 1).unwrap();

        // Directly seed pool reserves (bypass add_liquidity for bootstrap convenience)
        let pair_id = PairId::new(zbx(), zusd());
        let pair = dex.pool_factory.get_pair_mut(&pair_id).unwrap();
        pair.reserve_a = reserve;
        pair.reserve_b = reserve;
        pair.total_lp  = integer_sqrt(reserve * reserve).saturating_sub(MIN_LIQUIDITY);

        // Fund a test trader
        dex.allowances.add_balance(zbx(),  addr(10), 100_000);
        dex.allowances.add_balance(zusd(), addr(10), 100_000);
    }

    #[test]
    fn buy_zbx_for_zusd() {
        let mut dex = DexEngine::new();
        bootstrap(&mut dex, 1_000_000);

        let before = dex.balance_of(&zusd(), &addr(10));
        let event = dex.buy(
            addr(10), zbx(), zusd(),
            1_000, 0, 9999, 1, 1,
        ).unwrap();

        assert!(event.amount_out > 0);
        assert_eq!(dex.balance_of(&zbx(),  &addr(10)), 100_000 - 1_000);
        assert_eq!(dex.balance_of(&zusd(), &addr(10)), before + event.amount_out);
    }

    #[test]
    fn sell_is_alias_for_buy() {
        let mut dex = DexEngine::new();
        bootstrap(&mut dex, 1_000_000);

        let event = dex.sell(
            addr(10), zbx(), zusd(),
            500, 0, 9999, 1, 1,
        ).unwrap();
        assert!(event.amount_out > 0);
    }

    #[test]
    fn insufficient_balance_rejected() {
        let mut dex = DexEngine::new();
        bootstrap(&mut dex, 1_000_000);
        let r = dex.buy(addr(10), zbx(), zusd(), 200_000, 0, 9999, 1, 1);
        assert!(matches!(r, Err(DexError::InsufficientInputBalance { .. })));
    }

    #[test]
    fn no_route_returns_error() {
        let mut dex = DexEngine::new();
        let r = dex.buy(addr(10), addr(50), addr(51), 100, 0, 9999, 1, 1);
        assert!(matches!(r, Err(DexError::NoRoute)));
    }

    #[test]
    fn create_token_credits_supply_to_creator() {
        let mut dex = DexEngine::new();
        let tok_fee = FeeRegistry::default().token_creation_fee();
        let tok = dex.create_token(CreateTokenParams {
            name:                "MyToken".into(),
            symbol:              "MTK".into(),
            decimals:            18,
            total_supply:        5_000_000,
            max_supply:          0,
            mintable:            false,
            creator:             addr(5),
            creator_zbx_balance: tok_fee * 2,
            block_number:        1,
        }).unwrap();

        assert_eq!(dex.balance_of(&tok, &addr(5)), 5_000_000);
    }

    #[test]
    fn approve_and_transfer_from() {
        let mut dex = DexEngine::new();
        let tok = addr(10); let owner = addr(1); let spender = addr(2); let to = addr(3);
        dex.allowances.add_balance(tok, owner, 1_000);
        dex.approve(tok, owner, spender, 500, 0).unwrap();
        dex.transfer_from(tok, owner, to, spender, 300, 1).unwrap();
        assert_eq!(dex.balance_of(&tok, &to), 300);
        assert_eq!(dex.balance_of(&tok, &owner), 700);
    }

    #[test]
    fn quote_returns_estimate_without_mutating() {
        let mut dex = DexEngine::new();
        bootstrap(&mut dex, 1_000_000);
        let q = dex.quote(zbx(), zusd(), 1_000).unwrap();
        assert!(q.amount_out > 0);
        assert_eq!(q.hops, 1);
        // State is unchanged
        assert_eq!(dex.pool_factory.pairs[&PairId::new(zbx(), zusd())].reserve_a, 1_000_000);
    }

    #[test]
    fn fee_estimate_all_ops() {
        let dex = DexEngine::new();
        let est = dex.estimate_fee(DexOperation::SwapDirect, 15, 0);
        assert!(est.gas_cost_wei > 0);
        assert_eq!(est.platform_fee, 0); // swap has no platform fee
    }

    #[test]
    fn add_and_remove_liquidity_round_trip() {
        let mut dex = DexEngine::new();
        let pool_fee = dex.pool_factory.fee_registry().pool_creation_fee();
        dex.create_pool(zbx(), zusd(), FeeTier::Standard, addr(99), pool_fee * 2, 1).unwrap();

        let provider = addr(10);
        dex.allowances.add_balance(zbx(),  provider, 2_000_000);
        dex.allowances.add_balance(zusd(), provider, 2_000_000);

        // Add liquidity
        let (result, _evt) = dex.add_liquidity(
            provider, zbx(), zusd(),
            1_000_000, 1_000_000, 0, 9999, 1, 1,
        ).unwrap();
        assert!(result.lp_minted > 0);

        let lp = dex.lp_balance(zbx(), zusd(), &provider);
        assert_eq!(lp, result.lp_minted);

        // Remove half liquidity
        let half = lp / 2;
        let removed = dex.remove_liquidity(
            provider, zbx(), zusd(), half,
            0, 0, 9999, 1,
        ).unwrap();
        assert!(removed.amount_a > 0);
        assert!(removed.amount_b > 0);
    }
}
