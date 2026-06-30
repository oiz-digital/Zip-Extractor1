# ZEP-014 — AMM Pool Security: Canonical Trading Pairs with Production-Grade Guards

| Field | Value |
|---|---|
| **ZEP** | 014 |
| **Title** | AMM Pool Security — Canonical ZBX/ZUSD Trading Pair |
| **Author** | Zebvix Core Team |
| **Status** | DEPLOYED |
| **Category** | DeFi / Core |
| **Created** | 2026-05-03 |
| **Activation block** | 1 (genesis-deployed) |
| **Crate** | `zbx-pool` (v0.2) |

---

## Abstract

ZEP-014 defines the canonical AMM liquidity pool for ZBX Chain genesis (ZBX/ZUSD), introduces a comprehensive
10-layer security stack modelled on Uniswap v2 (fee formula) + Uniswap v3 (circuit breaker + oracle integration),
and specifies the multi-hop router for cross-pair swaps.

All pools are genesis-deployed at deterministic addresses. The AMM logic lives entirely in Rust
(`crates/zbx-pool`) — no Solidity required. This eliminates Solidity overflow, proxy, and upgrade
attack surfaces.

---

## Motivation

The original `zbx-pool` crate had 8 critical deficiencies:

| # | Deficiency | Impact |
|---|-----------|--------|
| D-1 | No fee deduction in swap formula | LPs earned 0 fees; pool economically broken |
| D-2 | `a * b` unchecked integer overflow | Silent result corruption on large reserves |
| D-3 | No slippage protection | Sandwich attacks trivially profitable |
| D-4 | No deadline | MEV bots could hold txs for hours |
| D-5 | No reentrancy guard | Flash-loan-powered recursive drain |
| D-6 | No price impact cap | Single tx could drain >90% of reserves |
| D-7 | No oracle deviation check | Manipulated pool price not detected |
| D-8 | No k-invariant check post-swap | Reserve accounting bugs invisible |

ZEP-014 fixes all 8, adds a circuit breaker, and ships a multi-hop router.

---

## Canonical Pools

The canonical pool is genesis-deployed at the following deterministic address:

| Pool | Token A | Token B | Fee | Address |
|------|---------|---------|-----|---------|
| **ZBX/ZUSD** | WZBX (`0x...0001`) | ZUSD (`0x...231D0001`) | **0.30%** | `0xAABB000100000000000000000000000000000001` |

**Fee tier rationale:**
- ZBX/ZUSD: 0.30% — ZBX is volatile; LPs bear impermanent loss risk.

---

## Token Canonical Addresses

```rust
// WZBX — wrapped native ZBX (ERC-20 interface over native gas token)
pub const WZBX_ADDR: [u8; 20] = [
    0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
    0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x01,
];

// ZUSD — native USD stablecoin (ZEP-002)
pub const ZUSD_ADDR: [u8; 20] = [
    0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
    0x00,0x00,0x00,0x00,0x00,0x00,0x23,0x1D,0x00,0x01,
];


```

---

## AMM Formula (Uniswap v2 with fee)

The constant-product formula with fee applied to `amount_in` before the swap:

```
fee_mult  = 10_000 − fee_bps
                          (e.g. 10_000 − 30 = 9_970 for 0.30%)
dx_fee    = amount_in × fee_mult
                          (amount after fee deduction, ×10_000 scale)
amount_out = dx_fee × reserve_out
             ─────────────────────────────────────────────────────
             (reserve_in × 10_000) + dx_fee
```

This is the exact Uniswap v2 formula. The ×10_000 scaling avoids floating point.

**k-invariant**: After every swap, the contract verifies:

```
new_k = (reserve_in + amount_in) × (reserve_out − amount_out) ≥ old_k
```

Any rounding error that violates this constraint causes an immediate revert.

---

## 10-Layer Security Stack

Every call to `swap()`, `add_liquidity()`, or `remove_liquidity()` runs these checks in order:

```
Step  Check                         Failure mode prevented
────  ──────────────────────────    ────────────────────────────────────────────
  1   CircuitBreaker::is_open()     Governance pause (emergency freeze)
  2   ReentrancyGuard::try_enter()  Flash-loan recursive re-entry
  3   now ≤ deadline                MEV bot holds tx for stale execution
  4   amount_in > 0                 Zero-value gas-burn
  5   Oracle deviation ≤ 15%        Pool-spot vs oracle price manipulation
  6   Price impact ≤ 30%            Single-tx reserve drain attack
  7   Uniswap v2 fee formula        Correct output (no fee bypass)
  8   amount_out ≤ 30% reserve_out  Reserve drain cap (complement to step 6)
  9   amount_out ≥ min_amount_out   Slippage protection (user-supplied bound)
 10   new_k ≥ old_k                 k-invariant (accounting correctness)
```

Checks are fail-fast: the first failure reverts the entire transaction.

---

## Swap API

```rust
pub fn swap(
    &mut self,
    token_in:       Address,   // must be token_a or token_b of this pool
    amount_in:      u128,      // raw units (not scaled)
    min_amount_out: u128,      // slippage bound — reverts if out < this
    deadline:       u64,       // Unix timestamp — reverts if now > deadline
    oracle_price:   Option<u128>, // pool oracle price for deviation check (None = skip)
    clock_now:      u64,       // injectable for testing
) -> Result<u128, AmmError>    // returns amount_out on success
```

---

## Add / Remove Liquidity API

```rust
// Add liquidity — returns LP tokens minted
pub fn add_liquidity(
    &mut self,
    amount_a:    u128,   // desired amount of token_a
    amount_b:    u128,   // desired amount of token_b (ratio-matched by pool)
    deadline:    u64,
    clock_now:   u64,
) -> Result<u128, AmmError>

// Remove liquidity — returns (amount_a, amount_b)
pub fn remove_liquidity(
    &mut self,
    lp_amount:   u128,
    deadline:    u64,
    clock_now:   u64,
) -> Result<(u128, u128), AmmError>
```

**MIN_LIQUIDITY = 1000**: On the very first `add_liquidity()`, 1000 LP units are permanently
burned (sent to address zero). This prevents the first-LP ownership attack where a single LP
could hold 100% of supply and extract value from subsequent LPs.

---

## Multi-Hop Router

```
find_best_route(token_in, token_out, amount_in) → Option<Route>

Route types:
  Direct (1-hop):        token_in → token_out
                         (only if canonical pair exists)

  2-hop via ZBX:         token_in → WZBX → token_out
  2-hop via ZUSD:        token_in → ZUSD  → token_out

Selection: simulate all applicable routes → return the one with highest amount_out
```

**`execute_route()`** validates and executes the chosen route atomically. If either hop
of a 2-hop route would fail any security check, the entire route reverts.

---

## Circuit Breaker

```rust
pub struct CircuitBreaker {
    pub paused: bool,
    pub last_pause_reason: Option<&'static str>,
}
```

Governance (via multisig) can call `pause(reason)` on any pool. All swap/liquidity operations
revert instantly when `paused = true`. Unpausing requires governance vote.

The oracle circuit breaker (Step 5 above) also auto-triggers when pool spot price deviates
from the ZEP-011 oracle price by more than 15%.

---

## Error Reference

| Error | Meaning |
|-------|---------|
| `AmmError::PoolPaused` | Circuit breaker active |
| `AmmError::Reentrancy` | Recursive call detected |
| `AmmError::Expired` | now > deadline |
| `AmmError::ZeroAmount` | amount_in = 0 |
| `AmmError::OracleDeviation` | Spot price > 15% from oracle |
| `AmmError::PriceImpactTooHigh` | This swap would move price > 30% |
| `AmmError::InsufficientOutput` | amount_out < min_amount_out |
| `AmmError::ReserveDrain` | amount_out > 30% of reserve |
| `AmmError::KInvariantViolated` | new_k < old_k (accounting bug) |
| `AmmError::InvalidToken` | Token not in this pair |
| `AmmError::Overflow` | Arithmetic overflow |
| `AmmError::InsufficientLiquidity` | Reserve too small for this trade |
| `AmmError::NoRoute` | Router found no 1-hop or 2-hop path |

---

## Genesis Seeding

At block 1, the genesis executor calls `apply_premint()` (ZEP-024, Session S24) and then
seeds the AMM pools from the Foundation Treasury allocation:

| Pool | ZBX side | Stable side | Starting price |
|------|----------|-------------|----------------|
| ZBX/ZUSD | 20,000,000 ZBX | 1,000,000 ZUSD | 1 ZUSD = 20 ZBX |

Seeding reserves are drawn from the 20,000,000 ZBX AMM genesis allocation (TOKENOMICS §Supply Allocation)
and the Foundation's ZUSD pre-mint.

---

## Security Analysis

| Attack | Mitigation |
|--------|-----------|
| Sandwich / frontrun | Slippage (`min_amount_out`) + deadline |
| Flash loan drain | Reentrancy guard + 30% reserve cap |
| Oracle price manipulation | 15% deviation check against ZEP-011 oracle |
| First-LP ownership attack | MIN_LIQUIDITY = 1000 LP units burned |
| Integer overflow | `checked_mul` everywhere + `safe_mul_div` |
| Governance rug | Circuit breaker only pauses, cannot drain; unpausing requires vote |
| Pool accounting bug | k-invariant check post-swap |
| MEV timing attack | Deadline parameter |
| Large trades breaking price | 30% price impact cap |

---

## Implementation

| File | Description |
|------|-------------|
| `crates/zbx-pool/src/error.rs` | 17 `AmmError` variants |
| `crates/zbx-pool/src/security.rs` | `ReentrancyGuard`, `CircuitBreaker`, 5 guard functions, 10 tests |
| `crates/zbx-pool/src/canonical_pairs.rs` | Token + pool addresses, `canonical_pools()`, helpers, 4 tests |
| `crates/zbx-pool/src/pair.rs` | `Pair` struct: `swap()`, `add_liquidity()`, `remove_liquidity()`, `get_amount_out()`, 14 tests |
| `crates/zbx-pool/src/router.rs` | `find_best_route()`, `execute_route()`, `simulate_route()`, 9 tests |
| `crates/zbx-pool/src/fee.rs` | `FeeTier` enum: Lowest(5), Low(10), Standard(30), High(100) in bps |

**Test count**: 39 unit tests across all pool modules.  
**Build status**: `cargo check` → 0 errors, 0 pool-specific warnings.

---

## Relation to Other ZEPs

| ZEP | Relation |
|-----|---------|
| ZEP-002 (ZUSD) | ZBX/ZUSD is a canonical pool; ZUSD peg supported by AMM arbitrage |
| ZEP-007 (TWAP) | AMM reserves are a primary TWAP source for the oracle |
| ZEP-008 (TWAP oracle) | Oracle price used in Step 5 oracle-deviation check |
| ZEP-011 (price oracle) | ZEP-011 feeds are the reference for AMM oracle deviation guard |
| ZEP-013 (ZINR) | WITHDRAWN — ZINR removed from ZBX Chain (Session 31) |
| ZEP-005 (ZUSD redemption) | ZUSD peg upper-bound via AMM; lower-bound via CDP redemption |

---

## Copyright

Copyright 2026 Zebvix Foundation. Licensed under CC0.
