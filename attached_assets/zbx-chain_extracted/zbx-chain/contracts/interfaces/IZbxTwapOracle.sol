// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxTwapOracle — Time-weighted average price oracle interface for ZbxAMM pairs.
/// @notice Consumes the Uniswap V2-style cumulative-price slots
///         (`price0CumulativeLast` / `price1CumulativeLast`) on each
///         ZbxAMM pair and exposes a per-pair, period-bounded TWAP query.
///         Closes AUDIT-2026-04-30 finding H-14 ("TWAP updates intra-block
///         → flash-loan oracle. DEFERRED — needs TWAP refactor.").
///
/// @dev    Cached-window design (S23a-fix1, post-architect MED-1):
///   1. Operator calls `registerPair(pair, period)` — seeds the baseline
///      `lastObservation` immediately. `primed = false` (consult will revert
///      until first window matures).
///   2. Anyone calls `update(pair)` — silently returns false (no state
///      change) if `block.timestamp - lastObservation.timestamp < period`.
///      Otherwise commits a fresh observation AND recomputes & caches the
///      time-weighted average over the just-elapsed window. Sets
///      `primed = true` after the first successful commit.
///   3. Anyone calls `consult(pair, tokenIn, amountIn)` — returns
///      `(cachedPriceAvg × amountIn) >> 112`. The cached price is ALWAYS
///      the time-weighted average over a window ≥ `period` (the actual
///      elapsed at the most recent commit). Reverts if `!primed`,
///      `!active`, or `tokenIn ∉ {token0, token1}`.
///
/// @custom:zbx-chain 8989
/// @custom:zep      8
interface IZbxTwapOracle {

    // ─── Errors ───────────────────────────────────────────────────────────

    error PairNotRegistered(address pair);
    error PairAlreadyRegistered(address pair);
    error PairInactive(address pair);
    error PeriodOutOfBounds(uint32 requested, uint32 min, uint32 max);
    error TokenNotInPair(address token, address pair);
    error NotPrimed(address pair);
    error ZeroPair();

    // ─── Events ───────────────────────────────────────────────────────────

    event PairRegistered(address indexed pair, uint32 period);
    event PairDeactivated(address indexed pair);
    event PeriodUpdated(address indexed pair, uint32 oldPeriod, uint32 newPeriod);
    /// @notice Emitted when `setPeriod` INCREASES the configured period
    ///         and the existing cached priceAvg (computed over the
    ///         shorter prior window) no longer satisfies the new
    ///         "cached window ≥ period" invariant. After this event,
    ///         `consult` reverts `NotPrimed` until the next successful
    ///         `update` matures a fresh window of length ≥ newPeriod.
    ///         (S23a-fix2)
    event PairCacheInvalidated(address indexed pair, uint32 oldPeriod, uint32 newPeriod);
    /// @notice Emitted on every successful `update` (and on the seed
    ///         observation written by `registerPair`). The seed emission
    ///         carries `windowSeconds = 0` because no prior baseline
    ///         existed against which to compute an average.
    event ObservationCommitted(
        address indexed pair,
        uint32          timestamp,
        uint256         price0Cumulative,
        uint256         price1Cumulative,
        uint32          windowSeconds,
        uint256         priceAvg0,
        uint256         priceAvg1
    );

    // ─── Operator surface (Ownable2Step.onlyOwner) ────────────────────────

    /// @notice Register a ZbxAMM pair for TWAP tracking and seed the
    ///         baseline observation. Pass `period == 0` to use
    ///         `DEFAULT_PERIOD`. After registration, at least ONE
    ///         successful `update` is required before `consult` works.
    function registerPair(address pair, uint32 period) external;

    /// @notice Adjust the look-back period of an already-registered pair.
    ///         Period DECREASE preserves the cached priceAvg (a
    ///         longer-window TWAP still satisfies a shorter window
    ///         requirement). Period INCREASE invalidates the cache
    ///         (`primed = false`, emits `PairCacheInvalidated`)
    ///         because the prior window is shorter than the new
    ///         requirement; `consult` reverts `NotPrimed` until the
    ///         next successful `update` matures a fresh window of
    ///         length ≥ newPeriod. (S23a-fix2)
    function setPeriod(address pair, uint32 period) external;

    /// @notice Deactivate a pair. After deactivation, `update` and
    ///         `consult` revert with `PairInactive`. Re-activation is
    ///         done via `registerPair` (which re-seeds and clears
    ///         `primed`).
    function deactivatePair(address pair) external;

    // ─── Permissionless surface ───────────────────────────────────────────

    /// @notice Commit a fresh observation if at least `period` seconds
    ///         have elapsed since the last one, AND recompute the cached
    ///         time-weighted average over that window. Returns false
    ///         (no revert, no state change) if the period has not
    ///         elapsed — safe to call from on-chain keepers every block.
    ///
    ///         Recommended keeper interval: `period / 2`.
    function update(address pair) external returns (bool committed);

    // ─── View surface ─────────────────────────────────────────────────────

    /// @notice Return the cached time-weighted output amount for swapping
    ///         `amountIn` of `tokenIn` through this pair. The TWAP is
    ///         computed at `update` time and cached; this view performs
    ///         only an SLOAD + multiplication.
    /// @dev   Math: `amountOut = (cachedPriceAvg × amountIn) >> 112`.
    ///        The cached priceAvg represents a window of length ≥
    ///        `period_at_commit_time`. After a `setPeriod` INCREASE,
    ///        the cache is invalidated until the next successful
    ///        `update`. Reverts `NotPrimed` (a) before any window has
    ///        matured, or (b) after `setPeriod` increased the period
    ///        and no fresh `update` has yet committed.
    function consult(address pair, address tokenIn, uint256 amountIn)
        external view returns (uint256 amountOut);

    /// @notice Per-pair configuration auto-getter (period, active, primed).
    function pairConfig(address pair)
        external view returns (uint32 period, bool active, bool primed);

    /// @notice Per-pair last-committed observation auto-getter
    ///         (timestamp, price0Cumulative, price1Cumulative).
    function lastObservation(address pair)
        external view returns (uint32 timestamp, uint256 price0Cumulative, uint256 price1Cumulative);

    /// @notice Per-pair cached TWAP averages (UQ112x112) from the most
    ///         recently completed window.
    function cachedAvg(address pair)
        external view returns (uint256 priceAvg0, uint256 priceAvg1);

    /// @notice Period bounds (constants).
    function MIN_PERIOD()     external pure returns (uint32);
    function MAX_PERIOD()     external pure returns (uint32);
    function DEFAULT_PERIOD() external pure returns (uint32);
}
