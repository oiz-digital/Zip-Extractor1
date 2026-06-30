// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { Ownable2Step }   from "./Ownable2Step.sol";
import { IZbxTwapOracle } from "./interfaces/IZbxTwapOracle.sol";

/// @dev Minimal subset of `ZbxAMM` that this oracle reads from.
///      Defined here (not imported from ZbxAMM.sol) to keep this file
///      cleanly decoupled from the AMM's full ABI surface.
interface IZbxAmmPair {
    function token0() external view returns (address);
    function token1() external view returns (address);
    function getReserves()
        external view returns (uint112 reserve0, uint112 reserve1, uint32 blockTimestampLast);
    function price0CumulativeLast() external view returns (uint256);
    function price1CumulativeLast() external view returns (uint256);
}

/// @title ZbxTwapOracle — Cached-window time-weighted-average price oracle for ZbxAMM pairs.
/// @author Zebvix Technologies Pvt Ltd
/// @notice One contract serves many pairs. Operator registers each pair
///         with a per-pair look-back `period` (5 min – 24 h, default
///         30 min). Permissionless `update(pair)`:
///           1. checks `elapsed >= period` (silent no-op otherwise)
///           2. computes `priceAvg = (cumNow - cumObs) / elapsed`
///           3. stores `priceAvg` in `cachedAvg[pair]`
///           4. updates `lastObservation[pair]` to the new baseline
///           5. emits `ObservationCommitted(...)`.
///         `consult(pair, tokenIn, amountIn)` is a pure SLOAD + shift —
///         it returns the cached priceAvg × amountIn, with NO on-the-fly
///         cumulative arithmetic. Because the cached priceAvg is computed
///         over a window ≥ `period`, a single-block flash spike
///         contributes at most `block_time / period` weight to the next
///         cached value (canonical Uni V2 SlidingWindowOracle guarantee).
///
/// @dev    Closes AUDIT-2026-04-30 finding H-14 ("contracts/ZbxAMM.sol:202
///         — TWAP updates intra-block → flash-loan oracle. DEFERRED").
///
/// @dev    S23a-fix1 design (post-architect MED-1): the original S23a
///         design had `consult` recompute `priceAvg = delta / elapsed`
///         on the fly, which meant the effective window could be ~1
///         block immediately after a fresh `update`, breaking the
///         period-length guarantee. The cached-window refactor enforces
///         the window invariant at write-time, not read-time.
///
/// @dev    Owner gating uses Ownable2Step (S18 base). `update` and
///         `consult` are PERMISSIONLESS; only setters are gated.
///
/// @custom:zbx-chain 8989
/// @custom:zep      8
contract ZbxTwapOracle is Ownable2Step, IZbxTwapOracle {

    // ─── S25-Y4 unchecked policy ─────────────────────────────────────────
    // All `unchecked { ... }` blocks in this contract fall into ONE of these
    // proven-safe categories (per S25 hardening pass):
    //   (a) post-require subtraction — preceding `require(x >= y)` proves
    //       `x - y` cannot underflow (token balance debits, allowance debits).
    //   (b) conservation pair — incrementing one slot by exactly the value
    //       just decremented (or vice-versa) from another, with the totalSupply
    //       invariant pre-checked (mint/burn/transfer leg of accounting pair).
    //   (c) bounded for-loop counter — `for (i; i < len; ) { ...; unchecked
    //       { i++; } }` where `len` is the bound; standard gas-saving pattern.
    //   (d) modular wrap intentional — uint32 timestamp/sequence wrap arithmetic
    //       (Uniswap V2 style); the wrap IS the spec.
    //   (e) UQ112x112 fixed-point shift — pre-bounded by uint112 reserve
    //       invariants (Uniswap V2 oracle accumulator).
    // Reviewers MUST classify any future `unchecked` block in this file
    // against one of (a)-(e) before merging; new categories require AUDIT entry.
    // ─────────────────────────────────────────────────────────────────────

    // ─── Constants ────────────────────────────────────────────────────────

    /// @inheritdoc IZbxTwapOracle
    uint32 public constant override MIN_PERIOD     = 5 minutes;
    /// @inheritdoc IZbxTwapOracle
    uint32 public constant override MAX_PERIOD     = 24 hours;
    /// @inheritdoc IZbxTwapOracle
    uint32 public constant override DEFAULT_PERIOD = 30 minutes;

    // ─── Storage ──────────────────────────────────────────────────────────

    struct PairConfig {
        uint32 period;
        bool   active;
        bool   primed;   // true once the cached priceAvg is valid (≥1 successful update)
    }

    struct Observation {
        uint32  timestamp;
        uint256 price0Cumulative;
        uint256 price1Cumulative;
    }

    struct CachedAvg {
        uint256 priceAvg0;   // UQ112x112 — fits in uint224, stored as uint256 for cleanliness
        uint256 priceAvg1;
    }

    /// @inheritdoc IZbxTwapOracle
    mapping(address => PairConfig)  public override pairConfig;
    /// @inheritdoc IZbxTwapOracle
    mapping(address => Observation) public override lastObservation;
    /// @inheritdoc IZbxTwapOracle
    mapping(address => CachedAvg)   public override cachedAvg;

    // ─── Constructor ──────────────────────────────────────────────────────

    /// @dev Bootstraps owner = `msg.sender`. To deploy with a different
    ///      bootstrap owner (e.g. multisig), wrap this contract in a
    ///      subclass with `constructor(address o) Ownable2Step(o) {}`.
    ///      Pattern mirrors `ZUSD.sol` and other S18-migrated contracts.
    constructor() Ownable2Step(msg.sender) {}

    // ─── Operator surface (Ownable2Step.onlyOwner) ────────────────────────

    /// @inheritdoc IZbxTwapOracle
    function registerPair(address pair, uint32 period) external override onlyOwner {
        if (pair == address(0))         revert ZeroPair();
        if (pairConfig[pair].active)    revert PairAlreadyRegistered(pair);
        if (period == 0)                period = DEFAULT_PERIOD;
        if (period < MIN_PERIOD || period > MAX_PERIOD)
            revert PeriodOutOfBounds(period, MIN_PERIOD, MAX_PERIOD);

        // Sanity probe: pair must respond to its expected ABI surface.
        // The two explicit token0()/token1() probes catch typo'd
        // addresses with a clear error. The subsequent _seedBaseline
        // call exercises getReserves() + price{0,1}CumulativeLast() via
        // _currentCumulativePrices, so the FULL pair ABI surface is
        // probed at registration time.
        IZbxAmmPair(pair).token0();
        IZbxAmmPair(pair).token1();

        pairConfig[pair] = PairConfig({ period: period, active: true, primed: false });
        emit PairRegistered(pair, period);

        // Seed the baseline observation. `primed` stays false — the cached
        // priceAvg is not yet meaningful; `consult` will revert NotPrimed
        // until the first `update` matures a window.
        _seedBaseline(pair);
    }

    /// @inheritdoc IZbxTwapOracle
    function setPeriod(address pair, uint32 period) external override onlyOwner {
        PairConfig storage cfg = pairConfig[pair];
        if (!cfg.active) revert PairNotRegistered(pair);
        if (period < MIN_PERIOD || period > MAX_PERIOD)
            revert PeriodOutOfBounds(period, MIN_PERIOD, MAX_PERIOD);

        uint32 oldPeriod = cfg.period;
        emit PeriodUpdated(pair, oldPeriod, period);
        cfg.period = period;

        // If period was INCREASED, the cached priceAvg (computed over
        // the shorter prior window) no longer satisfies the
        // "cached window ≥ period" invariant against the NEW period.
        // Invalidate the cache so `consult` reverts `NotPrimed` until
        // the next successful `update` commits a fresh window of
        // length ≥ newPeriod. Period DECREASE preserves the cache
        // because a longer-window TWAP still satisfies a shorter
        // window requirement.
        // S23a-fix2 — closes architect's NEW MED-1 finding.
        if (period > oldPeriod && cfg.primed) {
            cfg.primed = false;
            emit PairCacheInvalidated(pair, oldPeriod, period);
        }
    }

    /// @inheritdoc IZbxTwapOracle
    function deactivatePair(address pair) external override onlyOwner {
        PairConfig storage cfg = pairConfig[pair];
        if (!cfg.active) revert PairNotRegistered(pair);
        cfg.active = false;
        // primed is preserved across deactivate; if pair is re-registered,
        // registerPair will reset primed=false anyway. cachedAvg is also
        // left in place — re-registration overwrites it with fresh data.
        emit PairDeactivated(pair);
    }

    // ─── Permissionless surface ───────────────────────────────────────────

    /// @inheritdoc IZbxTwapOracle
    function update(address pair) external override returns (bool committed) {
        PairConfig memory cfg = pairConfig[pair];
        if (!cfg.active) revert PairInactive(pair);

        Observation memory obs = lastObservation[pair];
        uint32 nowTs = uint32(block.timestamp % 2**32);
        uint32 elapsed;
        unchecked { elapsed = nowTs - obs.timestamp; }   // wrap-safe per uint32 modular arithmetic

        if (elapsed < cfg.period) {
            return false;
        }

        // Compute cumulative prices INCLUDING in-progress accumulation
        // since the pair's own last `_update`. Mirrors canonical Uni V2
        // ExampleOracleSimple.
        (uint256 currCum0, uint256 currCum1, uint32 currTs) = _currentCumulativePrices(pair);

        // Cumulative subtraction is intentionally `unchecked` — the
        // cumulative slots are uint256 wrap-arithmetic accumulators
        // (Uni V2 invariant). Division by `elapsed` is safe (elapsed >=
        // cfg.period >= MIN_PERIOD = 5 min, never zero).
        uint256 priceAvg0;
        uint256 priceAvg1;
        unchecked {
            priceAvg0 = (currCum0 - obs.price0Cumulative) / elapsed;
            priceAvg1 = (currCum1 - obs.price1Cumulative) / elapsed;
        }

        // Persist new baseline + cached averages + primed flag in one
        // logical commit. Three SSTOREs at most (PairConfig.primed if
        // first commit, lastObservation, cachedAvg).
        cachedAvg[pair]        = CachedAvg({ priceAvg0: priceAvg0, priceAvg1: priceAvg1 });
        lastObservation[pair]  = Observation({
            timestamp:        currTs,
            price0Cumulative: currCum0,
            price1Cumulative: currCum1
        });
        if (!cfg.primed) {
            pairConfig[pair].primed = true;
        }

        emit ObservationCommitted(pair, currTs, currCum0, currCum1, elapsed, priceAvg0, priceAvg1);
        return true;
    }

    // ─── View surface ─────────────────────────────────────────────────────

    /// @inheritdoc IZbxTwapOracle
    function consult(address pair, address tokenIn, uint256 amountIn)
        external view override returns (uint256 amountOut)
    {
        PairConfig memory cfg = pairConfig[pair];
        if (!cfg.active) revert PairInactive(pair);
        if (!cfg.primed) revert NotPrimed(pair);

        IZbxAmmPair p = IZbxAmmPair(pair);
        address t0 = p.token0();
        address t1 = p.token1();
        if (tokenIn != t0 && tokenIn != t1) revert TokenNotInPair(tokenIn, pair);

        // Pure SLOAD + checked multiplication. `priceAvg` is UQ112x112
        // (≤ 2^224). For practical token amounts (< 2^150 wei) the
        // product fits comfortably in uint256. For pathologically huge
        // amountIn the checked multiplication reverts on overflow,
        // which is the correct (loud) failure mode.
        CachedAvg memory ca = cachedAvg[pair];
        uint256 priceAvg = (tokenIn == t0) ? ca.priceAvg0 : ca.priceAvg1;
        amountOut = (priceAvg * amountIn) >> 112;
    }

    // ─── Internals ────────────────────────────────────────────────────────

    /// @dev Seed the baseline observation at register time. Does NOT
    ///      compute or cache priceAvg (no prior baseline to average
    ///      against). Sets `lastObservation` only.
    function _seedBaseline(address pair) internal {
        (uint256 p0, uint256 p1, uint32 ts) = _currentCumulativePrices(pair);
        lastObservation[pair] = Observation({
            timestamp:        ts,
            price0Cumulative: p0,
            price1Cumulative: p1
        });
        emit ObservationCommitted(pair, ts, p0, p1, /* windowSeconds = */ 0, /* priceAvg0 = */ 0, /* priceAvg1 = */ 0);
    }

    /// @dev Compute the cumulative prices INCLUDING the in-progress
    ///      accumulation from the pair's last `_update` to now.
    ///      Mirrors the canonical Uniswap V2 oracle reference
    ///      implementation pattern.
    function _currentCumulativePrices(address pair)
        internal view returns (uint256 p0, uint256 p1, uint32 nowTs)
    {
        nowTs = uint32(block.timestamp % 2**32);
        IZbxAmmPair pp = IZbxAmmPair(pair);
        p0 = pp.price0CumulativeLast();
        p1 = pp.price1CumulativeLast();
        (uint112 r0, uint112 r1, uint32 lastTs) = pp.getReserves();
        if (lastTs != nowTs && r0 != 0 && r1 != 0) {
            uint32 timeElapsed;
            unchecked { timeElapsed = nowTs - lastTs; }
            unchecked {
                p0 += (uint256(r1) << 112) / r0 * timeElapsed;
                p1 += (uint256(r0) << 112) / r1 * timeElapsed;
            }
        }
    }

    // ─── EIP-165 (S21 chain) ──────────────────────────────────────────────

    function supportsInterface(bytes4 interfaceId) public pure virtual returns (bool) {
        return interfaceId == type(IZbxTwapOracle).interfaceId
            || interfaceId == 0x01ffc9a7;   // EIP-165 itself
    }
}
