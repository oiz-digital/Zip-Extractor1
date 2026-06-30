# ZEP-008 — Cached-Window Manipulation-Resistant TWAP Oracle

| Field | Value |
|---|---|
| **ZEP** | 008 |
| **Title** | Cached-window time-weighted average price oracle for ZbxAMM pairs |
| **Author** | Zebvix Technologies Pvt Ltd |
| **Status** | Draft (post-architect MED-1 fix) |
| **Type** | Standards (Contracts) |
| **Created** | 2026-05-02 |
| **Requires** | ZEP-007 (TVL oracle integration in S23b), Ownable2Step (S18 base) |
| **Closes** | AUDIT-2026-04-30 H-14 (`contracts/ZbxAMM.sol:202` — TWAP updates intra-block → flash-loan oracle. DEFERRED — needs TWAP refactor.) |

## Abstract

This proposal specifies `ZbxTwapOracle.sol`, a **cached-window** time-weighted-average-price oracle that consumes the Uniswap V2-style cumulative-price slots (`price0CumulativeLast` / `price1CumulativeLast`) on each `ZbxAMM` pair and exposes a per-pair, period-bounded TWAP query. The cached-window pattern enforces the look-back window invariant at write-time (in `update`) so that `consult` is a pure SLOAD + multiplication — guaranteeing every read uses an averaging window of at least `period` seconds. It closes AUDIT finding H-14, which flagged that consumers reading raw cumulative slots intra-block could be manipulated by single-block flash loans.

## Motivation

`ZbxAMM._update` (line ~202) correctly accumulates prices using Uniswap V2's verbatim formula:

```solidity
unchecked {
    price0CumulativeLast += (uint256(reserve1_) << 112) / reserve0_ * timeElapsed;
    price1CumulativeLast += (uint256(reserve0_) << 112) / reserve1_ * timeElapsed;
}
```

The math is correct. The vulnerability (H-14) is in *consumer behaviour*: any contract that reads `price0CumulativeLast` intra-block AND uses it as a price input is vulnerable to a flash-loan attacker who:

1. Takes a flash loan,
2. Swaps to spike spot price,
3. Calls the consumer (which observes the new cumulative state),
4. Reverses the swap,
5. Repays the flash loan.

The consumer sees the spiked price; everyone else sees the spike was fully reversed by end-of-block. The cumulative slots reflect the spike for whatever time elapsed, which can be 0 in a single-tx attack.

The defence is a *windowed* TWAP oracle that operator-snapshots cumulative state at periodic intervals and computes:

```
priceAverage = (cumulative_now - cumulative_snapshot) / (timestamp_now - timestamp_snapshot)
```

over a window long enough that single-block manipulation is amortised away. **The cached-window pattern enforces this window at write-time (in `update`), not at read-time (in `consult`)** — so the read path can never accidentally serve a sub-period window.

## Specification

### 3.1 Contract: `contracts/ZbxTwapOracle.sol`

```solidity
contract ZbxTwapOracle is Ownable2Step, IZbxTwapOracle {
    uint32 public constant MIN_PERIOD     = 5 minutes;
    uint32 public constant MAX_PERIOD     = 24 hours;
    uint32 public constant DEFAULT_PERIOD = 30 minutes;

    struct PairConfig  { uint32 period; bool active; bool primed; }
    struct Observation { uint32 timestamp; uint256 price0Cumulative; uint256 price1Cumulative; }
    struct CachedAvg   { uint256 priceAvg0; uint256 priceAvg1; }

    mapping(address => PairConfig)  public pairConfig;
    mapping(address => Observation) public lastObservation;
    mapping(address => CachedAvg)   public cachedAvg;
}
```

### 3.2 Operator surface (`Ownable2Step.onlyOwner`)

| Function | Purpose |
|---|---|
| `registerPair(address pair, uint32 period) external` | Register a `ZbxAMM` pair. Probes `token0`/`token1` to fail-fast on bad addresses; subsequent `_seedBaseline` exercises `getReserves` + `price{0,1}CumulativeLast` so the entire pair ABI is verified at registration. Seeds the baseline observation (`primed = false` until first `update` matures). `period == 0` ⇒ `DEFAULT_PERIOD`. |
| `setPeriod(address pair, uint32 period) external` | Adjust the look-back period. Must be in `[MIN_PERIOD, MAX_PERIOD]`. Period DECREASE preserves the cached priceAvg (a longer-window TWAP still satisfies a shorter requirement). Period INCREASE invalidates the cache (`primed = false`, emits `PairCacheInvalidated`) — `consult` reverts `NotPrimed` until the next successful `update` matures a window of length ≥ newPeriod. (S23a-fix2) |
| `deactivatePair(address pair) external` | Disable a pair. Subsequent `update` and `consult` revert with `PairInactive`. Re-activation is via `registerPair` (which clears `primed`). |

### 3.3 Permissionless surface

| Function | Purpose |
|---|---|
| `update(address pair) external returns (bool committed)` | If `block.timestamp - lastObservation.timestamp < period`: return `false` (no state change). Otherwise: compute `priceAvg{0,1} = (cumNow - cumObs) / elapsed`, store in `cachedAvg[pair]`, advance `lastObservation` to the new baseline, set `primed = true` if not already, emit `ObservationCommitted`. |

### 3.4 View surface

| Function | Purpose |
|---|---|
| `consult(address pair, address tokenIn, uint256 amountIn) external view returns (uint256 amountOut)` | Pure SLOAD + checked multiplication: `amountOut = (cachedPriceAvg × amountIn) >> 112`. The cached priceAvg represents a window of length ≥ `period_at_commit_time`. After a `setPeriod` INCREASE the cache is invalidated until the next successful `update`. Reverts `NotPrimed` either (a) before the first successful `update` post-register, or (b) after `setPeriod` increased period and no fresh `update` has yet committed. |

### 3.5 Errors

| Error | Trigger |
|---|---|
| `PairNotRegistered(address pair)` | `setPeriod`/`deactivatePair` called on never-registered pair |
| `PairAlreadyRegistered(address pair)` | `registerPair` called twice on same pair |
| `PairInactive(address pair)` | `update`/`consult` on deactivated pair |
| `PeriodOutOfBounds(uint32 requested, uint32 min, uint32 max)` | period < 5 min OR > 24 h |
| `TokenNotInPair(address token, address pair)` | `consult` with `tokenIn` ∉ {token0, token1} |
| `NotPrimed(address pair)` | `consult` before any window has matured (i.e., before the first successful `update` after register) |
| `ZeroPair()` | `registerPair(address(0), …)` |

### 3.6 Events

| Event | Trigger |
|---|---|
| `PairRegistered(address indexed pair, uint32 period)` | `registerPair` |
| `PairDeactivated(address indexed pair)` | `deactivatePair` |
| `PeriodUpdated(address indexed pair, uint32 oldPeriod, uint32 newPeriod)` | `setPeriod` |
| `PairCacheInvalidated(address indexed pair, uint32 oldPeriod, uint32 newPeriod)` | `setPeriod` when `newPeriod > oldPeriod` AND the pair was previously primed (S23a-fix2) |
| `ObservationCommitted(address indexed pair, uint32 timestamp, uint256 price0Cumulative, uint256 price1Cumulative, uint32 windowSeconds, uint256 priceAvg0, uint256 priceAvg1)` | `_seedBaseline` (with `windowSeconds = 0`, `priceAvg{0,1} = 0`) and `update` (with the just-elapsed window's data) |

### 3.7 EIP-165

`supportsInterface(type(IZbxTwapOracle).interfaceId)` returns `true`. Also advertises EIP-165 self.

## 4 Off-chain orchestration

A keeper service (zbx-indexer or any external bot) should call `update(pair)` once per `period` per registered pair. The function is gas-bounded (a few SLOADs from the pair plus three SSTOREs at most) and silently no-ops if called too frequently — no revert overhead, so an aggressive keeper poll loop is safe.

Recommended keeper interval: `period / 2` to ensure at least one successful commit per window even with mempool jitter or block-time variance.

**Freshness contract**: consumers that depend on the cached priceAvg being recent (e.g., liquidation engines) should additionally read `lastObservation(pair).timestamp` and apply their own staleness tolerance — the cached value is updated only at `update` time, not on every block.

## 5 Rationale

### 5.1 Cached-window vs lazy-window design

A naïve TWAP oracle computes `priceAvg = (cumNow - cumObs) / elapsed` at **read time** (`consult`). This has a subtle flaw: immediately after a fresh `update` commits, `elapsed` shrinks to a few seconds, and the effective averaging window collapses from `period` to ~1 block. An attacker who can land a flash spike + a `consult` call within a few seconds of an `update` extracts a price that is essentially the spot price.

The cached-window design closes this gap by computing `priceAvg` at **write time** (in `update`) and storing it. Because `update` only commits when `elapsed >= period`, the stored `priceAvg` is always averaged over a window of at least `period_at_commit_time` seconds. `consult` is then a pure SLOAD + multiplication and cannot accidentally serve a sub-period window.

**Period transition semantics (S23a-fix2)**: `setPeriod` to a LARGER value invalidates the cached priceAvg, because the prior commit was over a SHORTER window than the new requirement. `consult` reverts `NotPrimed` until the next successful `update` matures a fresh window of length ≥ newPeriod. `setPeriod` to a SMALLER value preserves the cache — a longer-window TWAP still satisfies the new shorter requirement. This preserves the "cached window ≥ current period" invariant under all owner-driven period adjustments.

### 5.2 Why a per-pair `period` (not a single global)

Pairs vary in liquidity. A deep ZBX/zUSD pool with `> $1M` reserves can safely use a 5-minute period (manipulation cost approaches the pool size). A thin long-tail pair needs ≥ 1 hour to push manipulation cost above attacker capital.

### 5.3 Why bounded `period` ∈ `[5 min, 24 h]`

- **Lower bound (5 min)**: prevents operator misconfiguration that would degenerate the TWAP into spot price (e.g., setting period = 0 or 30 sec would make a single block manipulation dominate the average).
- **Upper bound (24 h)**: prevents stale-price attacks where an operator sets period = 1 year and the oracle slowly drifts away from market price without anyone noticing.

### 5.4 Why permissionless `update`

If `update` were owner-gated, a key-compromised operator could freeze the TWAP at a stale value. Permissionless updates remove this single point of failure: anyone can refresh the observation when due, and the silent-no-op pattern means the update can be cheaply attempted on every block.

### 5.5 Why include in-progress accumulation in `_currentCumulativePrices`

The pair's `price0CumulativeLast` slot is only refreshed when someone calls `mint`/`burn`/`swap` on that pair. Between such events, the slot is stale by `block.timestamp - pair.blockTimestampLast` seconds. A TWAP committed without the in-progress portion would systematically under-weight the most recent period. The Uniswap V2 reference oracle pattern (which we mirror) computes the in-progress portion on-the-fly using the pair's current `getReserves()` to keep `update`-time TWAPs accurate up to the call moment.

### 5.6 Why use Uniswap V2's `2^112` scale (not OZ's `1e18`)

The cumulative slots in `ZbxAMM` already use the V2 `<<112` encoding. Switching scales would require either an extra division (precision loss) or rebuilding the AMM (out-of-scope). Mirroring the V2 oracle reference implementation lets us reuse existing audit literature and tooling.

### 5.7 Why `bool primed` instead of `timestamp == 0` sentinel

Using `obs.timestamp == 0` as "uninitialized" sentinel breaks at uint32 wrap (year 2106) when a legitimate timestamp can wrap to 0. The explicit `primed` flag in `PairConfig` is wrap-safe and reads more clearly.

## 6 Security considerations

### 6.1 Manipulation cost (cached-window guarantee)

For a pair with reserves `R₀, R₁` and `period = T` (at commit time — see §5.1 for `setPeriod` transition semantics), the cost for an attacker to move the cached priceAvg by ratio `α` from spot is determined by:

```
spike_weight_in_cached_priceAvg = spike_duration / T
```

For a 30-minute period and a 60-second held spike (5 EVM blocks at 12-sec block time), spike weight ≤ `60 / 1800 ≈ 3.3 %`. To move the cached priceAvg by 1 % the attacker must spike spot by ~30 % AND hold the spike for at least 60 sec — and the next `update` must fire mid-spike for the spike to be recorded at all. For a 2-block spike (24 sec), spike weight is `≤ 1.3 %`. For a 1-block spike (12 sec), spike weight is `≤ 0.67 %`.

This is the **canonical Uni V2 SlidingWindowOracle guarantee**, restored by S23a-fix1 after the original S23a was found to recompute `priceAvg` at read-time (defeating the guarantee).

### 6.2 Same-block update + consult

If a keeper calls `update(pair)` at time T, and an attacker calls `consult(pair, ...)` in the same block, the consult returns the cached priceAvg from the window that just CLOSED (i.e., `[T - period, T]`). The attacker cannot influence this window without already having held a spike for some fraction of `T - period` prior to T.

### 6.3 Pair deactivation

Operator may deactivate a pair (e.g., if the underlying AMM is exploited). Subsequent `consult` reverts immediately, preventing oracle consumers from continuing to trust a stale or compromised price.

### 6.4 Owner key risk

Operator setters use `Ownable2Step` (S18 base) — two-step ownership transfer (`transferOwnership` then `acceptOwnership`) prevents accidental transfer to a wrong address.

### 6.5 Pair sanity probe

`registerPair` calls `pair.token0()` and `pair.token1()` immediately. The follow-up `_seedBaseline` call exercises `getReserves()` + `price{0,1}CumulativeLast()` via `_currentCumulativePrices`, so the FULL pair ABI surface is exercised at registration time. A non-pair address fails to register.

### 6.6 Freshness vs cached-window trade-off

The cached priceAvg can be stale by up to `period` seconds (between `update` calls). For most use cases (TVL queries, fee distribution snapshots) this is acceptable. Time-critical consumers (liquidation engines, sub-second oracles) should compose with an additional staleness check by reading `lastObservation(pair).timestamp` and applying their own `maxAge` policy.

### 6.7 Out-of-scope (separately tracked)

| Item | Tracked as |
|---|---|
| TWAP-as-alt-price-source in `ZbxTvlOracle` (per-token toggle) | S23b ✅ DELIVERED — see ZEP-007 §3.7 |
| Multi-hop TWAP path (token X → ZBX → zUSD) | ZEP-008-FOLLOWUP-MULTIHOP |
| Off-chain keeper bot (Rust) for `update` calls | ZEP-008-FOLLOWUP-KEEPER |
| zbx-indexer subscribing to `ObservationCommitted` events | ZEP-008-FOLLOWUP-INDEXER |
| Auto-staleness `maxAge` per-pair check inside `consult` | ZEP-008-FOLLOWUP-MAXAGE (only if a consumer needs it) |

## 7 Reference implementation

- `contracts/interfaces/IZbxTwapOracle.sol` — interface (~110 LOC)
- `contracts/ZbxTwapOracle.sol` — implementation (~245 LOC)
- `contracts/test/ZbxTwapOracle.t.sol` — HEVM test suite (17 tests + 1 mock pair)

## 8 Off-sandbox verification (mandatory before deploy)

- `forge build --sizes` — confirm bytecode within 24 KB limit (estimated ~2.5 KB).
- `forge test --match-path contracts/test/ZbxTwapOracle.t.sol -vvv` — all 17 tests PASS.
- `slither contracts/ZbxTwapOracle.sol` — expect zero new HIGH/MED findings.
- Mythril symbolic execution on `consult` to verify no integer overflow paths in priceAvg × amountIn.
- Multi-block fuzz on testnet 8990: deploy, register a real ZBX/zUSD pair, run a keeper bot for 24 h, and verify the cached priceAvg tracks spot to within ±2 % under benign conditions.

## 9 Reference

- Uniswap V2 oracle reference implementation: <https://github.com/Uniswap/v2-periphery/blob/master/contracts/examples/ExampleSlidingWindowOracle.sol>
- ZEP-007 (TVL oracle): integration LIVE per S23b — see ZEP-007 §3.7 for the routing semantics, setter invariants, and fail-closed policy.
- AUDIT-2026-04-30 H-14: deferred finding closed by this ZEP.
- S23a-fix1 architect feedback (MED-1): cached-window refactor rationale.
