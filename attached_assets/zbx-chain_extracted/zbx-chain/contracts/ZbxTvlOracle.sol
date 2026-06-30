// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { IZbxTvlOracle } from "./interfaces/IZbxTvlOracle.sol";
import { Ownable2Step } from "./Ownable2Step.sol";

// ─── Minimal external interfaces (no cross-contract imports) ────────────────

interface IAggregatorV3 {
    function decimals() external view returns (uint8);
    function latestRoundData() external view returns (
        uint80  roundId,
        int256  answer,
        uint256 startedAt,
        uint256 updatedAt,
        uint80  answeredInRound
    );
}

interface IZRC20Lite {
    function decimals() external view returns (uint8);
}

interface IZbxAMMFactoryLite {
    function allPairsLength() external view returns (uint256);
    function allPairs(uint256 idx) external view returns (address);
}

interface IZbxAMMPairLite {
    function token0() external view returns (address);
    function token1() external view returns (address);
    function getReserves() external view returns (uint112, uint112, uint32);
}

interface IZbxLendingPoolLite {
    function reservesCount() external view returns (uint256);
    function reserveList(uint256 idx) external view returns (address);
    /// Wrapper view added to ZbxLendingPool in S17-T03. Returns the entire
    /// ReserveData struct so the oracle does not need to decode the 18-tuple
    /// public-mapping auto-getter.
    function getReserveData(address asset) external view returns (
        address asset_,
        address zToken,
        address debtToken,
        uint128 totalSupplied,    // SCALED
        uint128 totalBorrowed,    // SCALED
        uint128 liquidityRate,
        uint128 borrowRate,
        uint128 liquidityIndex,   // RAY (1e27)
        uint128 borrowIndex,      // RAY (1e27)
        uint40  lastUpdateTimestamp,
        uint16  ltv,
        uint16  liquidationThreshold,
        uint16  liquidationBonus,
        uint16  reserveFactor,
        uint8   decimals,
        bool    active,
        bool    borrowEnabled,
        bool    flashLoanEnabled
    );
}

interface IZusdStabilityPoolLite {
    function totalDeposits() external view returns (uint256);
}

interface IZRC20StakingLite {
    function totalStaked() external view returns (uint256);
    function stakingToken() external view returns (address);
}

/// @notice Minimal subset of IZbxTwapOracle (ZEP-008) consumed for the
///         per-token alt-price-source path. Implementing oracle MUST
///         conform to the cached-window guarantee (cached priceAvg over
///         a window of length ≥ period_at_commit_time). See ZEP-008 §5.1.
///         (S23b)
interface IZbxTwapOracleLite {
    function consult(address pair, address tokenIn, uint256 amountIn)
        external view returns (uint256 amountOut);
}

/// @notice S24 — Phase 7 REWARD source: minimal subset of
///         IZbxRewardDistributor consumed by `_tvlReward`. The TVL of
///         the reward pool is the underlying ZBX-token balance held by
///         the distributor (== sum of un-claimed `pendingRewards` since
///         claims `transfer()` ZBX out of the distributor). We only
///         need the `zbx()` getter to derive which ERC-20 to query.
interface IZbxRewardDistributorLite {
    function zbx() external view returns (address);
}

/// @notice S24 — Phase 7 BRIDGE_VAULT source: minimal subset of
///         IBridgeVault consumed by `_tvlBridgeVault`. BridgeVault is
///         single-token (immutable `token`) and exposes the locked
///         total directly via `totalLocked`. NOTE: this is a single
///         IBridgeVault per chain; multi-token aggregation across
///         multiple bridge vaults is out-of-scope for v1 (separately
///         tracked).
interface IBridgeVaultLite {
    function token()       external view returns (address);
    function totalLocked() external view returns (uint256);
}

/// @notice S24 — Minimal IERC20.balanceOf for reading on-chain token
///         balances held by source contracts (reward distributor,
///         future per-token vaults). We deliberately do NOT pull the
///         full IERC20 surface to avoid coupling to a particular
///         interface variant.
interface IERC20BalanceOf {
    function balanceOf(address account) external view returns (uint256);
}

/// @title ZbxTvlOracle — On-chain Total Value Locked aggregator for Zebvix Chain.
///
/// @notice Reports protocol TVL in USD with 18-decimal precision by summing
///         contributions from every configured liquidity source. Read-only:
///         no state mutations from `tvl*()` paths beyond updating the
///         transient `_unpricedTokens` set (cleared on each entry).
///
/// @dev Decimals normalization (canonical formula):
///        usd18 = amount × price × 10^(18 - tokenDec - priceDec)   if total ≤ 18
///        usd18 = amount × price ÷ 10^(tokenDec + priceDec - 18)   if total > 18
///
///      Stale-price policy: a per-token price with `block.timestamp -
///      updatedAt > maxStaleness` is treated as "missing". The token is
///      added to `_unpricedTokens` and contributes ZERO to TVL. This is
///      fail-closed: we'd rather under-report than feed a stale number to
///      an off-chain dashboard.
///
///      Pair-scan cap: `tvlAMM()` iterates at most `maxPairsToScan` pairs
///      from the factory's `allPairs` array (default 256). Operators MUST
///      raise the cap as the pair count grows; any pair beyond the cap is
///      silently excluded (intentional gas-DoS protection).
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:standard   ZEP-007-TVL-ORACLE v1
contract ZbxTvlOracle is IZbxTvlOracle, Ownable2Step {

    // ─── Storage ──────────────────────────────────────────────────────────

    bool    public override paused;

    /// @dev token => Chainlink-style aggregator address.
    mapping(address => address) public override priceFeed;

    /// @dev Source enum index => contract address.
    mapping(uint8 => address) private _sources;

    uint64 public override maxStaleness   = 3600;   // 1 hour
    uint16 public override maxPairsToScan = 256;

    /// @dev Transient list of tokens encountered without a price feed during
    ///      the most recent aggregation. Reset at the top of each aggregation
    ///      entry-point. Solidity 0.8 has no native transient storage in
    ///      ^0.8.24, so we use a normal storage array and accept the SSTORE
    ///      cost; values are always-overwritten and never grow unbounded
    ///      (capped by maxPairsToScan + reservesCount + a few extras).
    address[] private _unpricedTokens;

    uint256 private constant RAY = 1e27;

    /// @notice Operator-registered deposit token for the STABILITY source.
    /// @dev    The stability pool's deposit token (ZUSD on mainnet) is not
    ///         discoverable from the pool's external surface, so the operator
    ///         must register it explicitly via `setStabilityDepositToken`.
    ///         Set to `address(0)` to disable the stability source.
    address public stabilityDepositToken;

    // ─── S23b — TWAP alt-price-source (ZEP-008 integration) ──────────────
    //
    // Per-token toggle: the operator may opt a token into a TWAP-routed
    // price path instead of the default ZbxAggregatorV3 (Chainlink-style)
    // feed. The routing is single-hop by construction:
    //
    //   token  ──TWAP.consult(pair, token, amt)──▶  amt-in-quoteToken
    //   quoteToken ──aggregator.latestRoundData()──▶  USD-18
    //
    // The setter (`setTwapRoute`) enforces at config-time that
    // `quoteToken` has a working `priceFeed` aggregator registered, so
    // the path always terminates at the aggregator (no nested TWAP, no
    // recursion, no depth limit needed). At runtime, if the aggregator
    // for `quoteToken` is later un-registered or goes stale, the path
    // fail-closes to ZERO contribution (consistent with the existing
    // missing/stale-feed policy in `_safeUSDAggregatorOnly`).
    //
    // The setter ALSO verifies that the supplied `pair` actually
    // contains both `token` and `quoteToken` via `pair.token0`/`token1`,
    // catching operator typos that would otherwise quietly route a
    // token's price through an unrelated pool.

    IZbxTwapOracleLite public twapOracle;

    struct TwapRoute {
        address pair;
        address quoteToken;
        bool    enabled;
    }

    mapping(address => TwapRoute) public twapRoute;

    // ─── Constructor ──────────────────────────────────────────────────────
    //
    // Two-step ownership inherited from Ownable2Step (S18). The base
    // constructor reverts if `owner_ == address(0)`, so the previous
    // ZeroAddress() check has been removed as dead code.
    constructor(address owner_) Ownable2Step(owner_) {}

    // ─── Modifiers ────────────────────────────────────────────────────────
    //
    // `onlyOwner` and `NotOwner` come from Ownable2Step.

    modifier whenNotPaused() {
        if (paused) revert PausedQuery();
        _;
    }

    // ─── Aggregated views ─────────────────────────────────────────────────

    function totalValueLockedUSD()
        external
        view
        override
        whenNotPaused
        returns (uint256)
    {
        return _tvlAMM() + _tvlLending() + _tvlStability() + _tvlStaking()
             + _tvlReward() + _tvlBridgeVault();
    }

    function tvlBySource(Source src)
        external
        view
        override
        whenNotPaused
        returns (uint256)
    {
        if (src == Source.AMM)          return _tvlAMM();
        if (src == Source.LENDING)      return _tvlLending();
        if (src == Source.STABILITY)    return _tvlStability();
        if (src == Source.STAKING)      return _tvlStaking();
        if (src == Source.REWARD)       return _tvlReward();
        if (src == Source.BRIDGE_VAULT) return _tvlBridgeVault();
        revert UnknownSource();
    }

    function tvlByToken(address token)
        external
        view
        override
        whenNotPaused
        returns (uint256 totalUsd)
    {
        // AMM: sum reserves of `token` across every pair where it appears.
        address factory = _sources[uint8(Source.AMM)];
        if (factory != address(0)) {
            uint256 nPairs = IZbxAMMFactoryLite(factory).allPairsLength();
            uint256 cap    = nPairs < maxPairsToScan ? nPairs : maxPairsToScan;
            for (uint256 i; i < cap; ++i) {
                address pair = IZbxAMMFactoryLite(factory).allPairs(i);
                (address t0, address t1, uint112 r0, uint112 r1) = _readPair(pair);
                if (t0 == token) totalUsd += _safeUSD(token, uint256(r0));
                if (t1 == token) totalUsd += _safeUSD(token, uint256(r1));
            }
        }

        // LENDING: net real liquidity of `token` in the lending pool.
        address pool = _sources[uint8(Source.LENDING)];
        if (pool != address(0)) {
            try IZbxLendingPoolLite(pool).getReserveData(token) returns (
                address, address, address,
                uint128 supplied, uint128 borrowed,
                uint128, uint128,
                uint128 liqIdx, uint128 borIdx,
                uint40, uint16, uint16, uint16, uint16,
                uint8, bool active, bool, bool
            ) {
                if (active) {
                    uint256 realSupplied = (uint256(supplied) * uint256(liqIdx)) / RAY;
                    uint256 realBorrowed = (uint256(borrowed) * uint256(borIdx)) / RAY;
                    if (realSupplied > realBorrowed) {
                        totalUsd += _safeUSD(token, realSupplied - realBorrowed);
                    }
                }
            } catch { /* token not a reserve — skip */ }
        }
    }

    function tvlBreakdown()
        external
        view
        override
        whenNotPaused
        returns (TvlBreakdown memory b)
    {
        b.amm         = _tvlAMM();
        b.lending     = _tvlLending();
        b.stability   = _tvlStability();
        b.staking     = _tvlStaking();
        b.reward      = _tvlReward();
        b.bridgeVault = _tvlBridgeVault();
        b.total       = b.amm + b.lending + b.stability + b.staking + b.reward + b.bridgeVault;
        b.timestamp   = block.timestamp;
    }

    // ─── Per-source view wrappers ─────────────────────────────────────────

    function tvlAMM()         external view override whenNotPaused returns (uint256) { return _tvlAMM(); }
    function tvlLending()     external view override whenNotPaused returns (uint256) { return _tvlLending(); }
    function tvlStability()   external view override whenNotPaused returns (uint256) { return _tvlStability(); }
    function tvlStaking()     external view override whenNotPaused returns (uint256) { return _tvlStaking(); }
    function tvlReward()      external view override whenNotPaused returns (uint256) { return _tvlReward(); }
    function tvlBridgeVault() external view override whenNotPaused returns (uint256) { return _tvlBridgeVault(); }

    function source(Source src) external view override returns (address) {
        return _sources[uint8(src)];
    }

    function unpricedTokens() external view override returns (address[] memory) {
        return _unpricedTokens;
    }

    /// @notice Off-chain monitoring view: how many AMM pairs the factory
    ///         currently exposes vs how many `tvlAMM()` is willing to
    ///         scan, and whether truncation occurred.
    /// @dev    A `truncated == true` reading means the reported
    ///         `tvlAMM()` value is a lower bound; operator should raise
    ///         `maxPairsToScan` (and accept the higher gas cost) or
    ///         shard scanning across multiple oracle deployments.
    function pairScanStats()
        external
        view
        override
        returns (uint256 totalPairs, uint256 scanned, bool truncated)
    {
        address factory = _sources[uint8(Source.AMM)];
        if (factory == address(0)) return (0, 0, false);
        totalPairs = IZbxAMMFactoryLite(factory).allPairsLength();
        scanned    = totalPairs < maxPairsToScan ? totalPairs : maxPairsToScan;
        truncated  = totalPairs > maxPairsToScan;
    }

    // ─── Internal aggregations ────────────────────────────────────────────

    function _tvlAMM() internal view returns (uint256 usd) {
        address factory = _sources[uint8(Source.AMM)];
        if (factory == address(0)) return 0;

        uint256 nPairs = IZbxAMMFactoryLite(factory).allPairsLength();
        uint256 cap    = nPairs < maxPairsToScan ? nPairs : maxPairsToScan;

        for (uint256 i; i < cap; ++i) {
            address pair = IZbxAMMFactoryLite(factory).allPairs(i);
            (address t0, address t1, uint112 r0, uint112 r1) = _readPair(pair);
            usd += _safeUSD(t0, uint256(r0));
            usd += _safeUSD(t1, uint256(r1));
        }
    }

    function _tvlLending() internal view returns (uint256 usd) {
        address pool = _sources[uint8(Source.LENDING)];
        if (pool == address(0)) return 0;

        uint256 n = IZbxLendingPoolLite(pool).reservesCount();
        for (uint256 i; i < n; ++i) {
            address asset = IZbxLendingPoolLite(pool).reserveList(i);
            (
                , , ,
                uint128 supplied, uint128 borrowed,
                , ,
                uint128 liqIdx, uint128 borIdx,
                , , , , ,
                , bool active, ,
            ) = IZbxLendingPoolLite(pool).getReserveData(asset);
            if (!active) continue;

            uint256 realSupplied = (uint256(supplied) * uint256(liqIdx)) / RAY;
            uint256 realBorrowed = (uint256(borrowed) * uint256(borIdx)) / RAY;
            if (realSupplied > realBorrowed) {
                usd += _safeUSD(asset, realSupplied - realBorrowed);
            }
        }
    }

    function _tvlStability() internal view returns (uint256) {
        address pool = _sources[uint8(Source.STABILITY)];
        if (pool == address(0)) return 0;
        if (stabilityDepositToken == address(0)) return 0;

        uint256 amount = IZusdStabilityPoolLite(pool).totalDeposits();
        if (amount == 0) return 0;
        return _safeUSD(stabilityDepositToken, amount);
    }

    function _tvlStaking() internal view returns (uint256) {
        address staking = _sources[uint8(Source.STAKING)];
        if (staking == address(0)) return 0;

        uint256 amount = IZRC20StakingLite(staking).totalStaked();
        if (amount == 0) return 0;

        // staking.stakingToken() is required by ZRC20Staking; if the call
        // reverts (older deployment) we cannot price the stake — return 0.
        try IZRC20StakingLite(staking).stakingToken() returns (address tok) {
            return _safeUSD(tok, amount);
        } catch { return 0; }
    }

    function _tvlReward() internal view returns (uint256) {
        // S24 — Phase 7 REAL IMPLEMENTATION (was scaffolded `pure` returning 0).
        //
        // The reward pool's TVL is the underlying ZBX-token balance held by
        // the configured `ZbxRewardDistributor`. Claims `transfer()` ZBX out
        // of the distributor, so its `balanceOf` is exactly the un-distributed
        // (== still-locked-up-for-rewards) amount.
        //
        // Operator wires this via the existing canonical
        // `setSource(Source.REWARD, distributor)` admin path (Ownable2Step.onlyOwner).
        // Un-wiring (set to address(0)) makes this source return 0 cleanly.
        //
        // Fail-closed contract:
        //   - distributor un-wired           → 0
        //   - `zbx()` reverts or returns 0   → 0 (try/catch + zero-guard)
        //   - `balanceOf(distributor)` reverts → 0 (try/catch)
        //   - ZBX has no aggregator priceFeed → `_safeUSD` returns 0
        //     (existing fail-closed policy)
        //
        // Cannot revert the surrounding tvlBreakdown / tvlGlobal call.
        address dist = _sources[uint8(Source.REWARD)];
        if (dist == address(0)) return 0;

        address zbxTok;
        try IZbxRewardDistributorLite(dist).zbx() returns (address z) {
            zbxTok = z;
        } catch { return 0; }
        if (zbxTok == address(0)) return 0;

        uint256 bal;
        try IERC20BalanceOf(zbxTok).balanceOf(dist) returns (uint256 b) {
            bal = b;
        } catch { return 0; }
        if (bal == 0) return 0;

        return _safeUSD(zbxTok, bal);
    }

    function _tvlBridgeVault() internal view returns (uint256) {
        // S24 — Phase 7 REAL IMPLEMENTATION (was scaffolded `pure` returning 0).
        //
        // The bridge vault's TVL is the configured BridgeVault's `totalLocked`
        // (in `token` units), priced via `_safeUSD`. BridgeVault is single-token
        // (immutable `token`) — multi-token aggregation across multiple bridge
        // vaults is out-of-scope for v1 (separately tracked as
        // S24-FOLLOWUP-MULTIVAULT).
        //
        // Operator wires this via the existing canonical
        // `setSource(Source.BRIDGE_VAULT, vault)` admin path (Ownable2Step.onlyOwner).
        //
        // Fail-closed contract: every external call is in try/catch; un-wired
        // address, missing token, or unpriced token all yield 0. Cannot revert
        // the surrounding tvlBreakdown / tvlGlobal call.
        address vault = _sources[uint8(Source.BRIDGE_VAULT)];
        if (vault == address(0)) return 0;

        address tok;
        try IBridgeVaultLite(vault).token() returns (address t) {
            tok = t;
        } catch { return 0; }
        if (tok == address(0)) return 0;

        uint256 locked;
        try IBridgeVaultLite(vault).totalLocked() returns (uint256 l) {
            locked = l;
        } catch { return 0; }
        if (locked == 0) return 0;

        return _safeUSD(tok, locked);
    }

    // ─── Internal helpers ─────────────────────────────────────────────────

    function _readPair(address pair)
        internal
        view
        returns (address t0, address t1, uint112 r0, uint112 r1)
    {
        t0 = IZbxAMMPairLite(pair).token0();
        t1 = IZbxAMMPairLite(pair).token1();
        (r0, r1, ) = IZbxAMMPairLite(pair).getReserves();
    }

    /// @notice Convert `amount` of `token` to USD-18.
    /// @dev    Routing:
    ///           - If `twapRoute[token].enabled` AND `twapOracle` is set:
    ///             call `twapOracle.consult(pair, token, amount)` to obtain
    ///             the amount denominated in `quoteToken`, then resolve via
    ///             the quote token's aggregator feed.
    ///           - Otherwise: resolve `token`'s price directly via its
    ///             aggregator feed.
    ///         Both paths are fail-closed: missing/stale/negative price,
    ///         out-of-policy decimals, or a TWAP `consult` revert (e.g.
    ///         `NotPrimed`, `PairInactive`) all return 0. View functions
    ///         cannot mutate `_unpricedTokens`; that list is populated via
    ///         the explicit `refreshUnpriced()` mutator path. (S23b)
    function _safeUSD(address token, uint256 amount) internal view returns (uint256) {
        if (amount == 0 || token == address(0)) return 0;

        TwapRoute memory r = twapRoute[token];
        if (r.enabled) {
            if (address(twapOracle) == address(0)) return 0; // fail-closed
            try twapOracle.consult(r.pair, token, amount) returns (uint256 quoteAmt) {
                // Single hop: TWAP -> aggregator. The setter enforced at
                // config time that quoteToken has an aggregator feed, so
                // recursion / depth limits are unnecessary.
                return _safeUSDAggregatorOnly(r.quoteToken, quoteAmt);
            } catch {
                return 0;
            }
        }

        return _safeUSDAggregatorOnly(token, amount);
    }

    /// @notice Aggregator-only USD resolution (the legacy pre-S23b path,
    ///         extracted as a helper so the TWAP branch can re-use it for
    ///         the quote-token leg).
    function _safeUSDAggregatorOnly(address token, uint256 amount)
        internal
        view
        returns (uint256)
    {
        if (amount == 0 || token == address(0)) return 0;

        address feed = priceFeed[token];
        if (feed == address(0)) return 0;

        try IAggregatorV3(feed).latestRoundData() returns (
            uint80, int256 price, uint256, uint256 updatedAt, uint80
        ) {
            if (price <= 0) return 0;
            if (updatedAt + maxStaleness < block.timestamp) return 0;

            uint8 priceDec;
            try IAggregatorV3(feed).decimals() returns (uint8 d) { priceDec = d; }
            catch { priceDec = 8; }

            uint8 tokenDec;
            try IZRC20Lite(token).decimals() returns (uint8 d) { tokenDec = d; }
            catch { tokenDec = 18; }

            return _normalize18(amount, tokenDec, uint256(price), priceDec);
        } catch {
            return 0;
        }
    }

    /// @notice Hard upper bound on accepted token decimals.
    /// @dev    Standard ERC-20 caps at 18. We allow up to 36 for exotic
    ///         tokens (e.g. some wrapped staking derivatives). Beyond that,
    ///         `10 ** (totalIn - 18)` exponents grow toward uint256 limits
    ///         and the math becomes nonsensical for TVL aggregation.
    uint8 internal constant MAX_TOKEN_DECIMALS = 36;

    /// @notice Hard upper bound on accepted aggregator decimals.
    /// @dev    Chainlink standard is 8 (USD pairs) or 18 (ETH pairs). We
    ///         allow up to 18 with a wide margin for first-party feeds.
    uint8 internal constant MAX_PRICE_DECIMALS = 18;

    function _normalize18(
        uint256 amount,
        uint8   tokenDec,
        uint256 price,
        uint8   priceDec
    ) internal pure returns (uint256) {
        // Bounds guard — fail-closed for out-of-policy decimal configs to
        // prevent overflow / divide-by-zero in the exponentiation paths.
        if (tokenDec > MAX_TOKEN_DECIMALS) return 0;
        if (priceDec > MAX_PRICE_DECIMALS) return 0;

        uint256 totalIn = uint256(tokenDec) + uint256(priceDec);
        if (totalIn == 18) {
            return amount * price;
        } else if (totalIn > 18) {
            return (amount * price) / (10 ** (totalIn - 18));
        } else {
            return amount * price * (10 ** (18 - totalIn));
        }
    }

    /// @notice Mutator twin of the view aggregations: walks every source
    ///         with the same logic as `totalValueLockedUSD()` and records
    ///         tokens lacking a working price feed into `_unpricedTokens`.
    ///         Does NOT return the aggregated USD value — call the views
    ///         for that. Cleared at the start of every call.
    function refreshUnpriced() external override whenNotPaused {
        delete _unpricedTokens;

        // AMM
        address factory = _sources[uint8(Source.AMM)];
        if (factory != address(0)) {
            uint256 nPairs = IZbxAMMFactoryLite(factory).allPairsLength();
            uint256 cap    = nPairs < maxPairsToScan ? nPairs : maxPairsToScan;
            if (nPairs > cap) emit PairScanTruncated(nPairs, cap);
            for (uint256 i; i < cap; ++i) {
                address pair = IZbxAMMFactoryLite(factory).allPairs(i);
                (address t0, address t1, , ) = _readPair(pair);
                _checkPriced(t0);
                _checkPriced(t1);
            }
        }

        // LENDING
        address pool = _sources[uint8(Source.LENDING)];
        if (pool != address(0)) {
            uint256 n = IZbxLendingPoolLite(pool).reservesCount();
            for (uint256 i; i < n; ++i) {
                address asset = IZbxLendingPoolLite(pool).reserveList(i);
                _checkPriced(asset);
            }
        }

        // STABILITY
        if (stabilityDepositToken != address(0)) {
            _checkPriced(stabilityDepositToken);
        }

        // STAKING
        address staking = _sources[uint8(Source.STAKING)];
        if (staking != address(0)) {
            try IZRC20StakingLite(staking).stakingToken() returns (address tok) {
                _checkPriced(tok);
            } catch {}
        }

        // S24 — REWARD: monitor the distributor's ZBX feed. Mirrors
        // _tvlReward's lite-interface fail-closed contract: any
        // distributor-side revert is swallowed and the pricing
        // dependency simply isn't recorded (the surface symptom would
        // be tvlReward = 0, which is the intended fail-closed posture).
        address dist = _sources[uint8(Source.REWARD)];
        if (dist != address(0)) {
            try IZbxRewardDistributorLite(dist).zbx() returns (address tok) {
                _checkPriced(tok);
            } catch {}
        }

        // S24 — BRIDGE_VAULT: monitor the vault's underlying token feed.
        address vault = _sources[uint8(Source.BRIDGE_VAULT)];
        if (vault != address(0)) {
            try IBridgeVaultLite(vault).token() returns (address tok) {
                _checkPriced(tok);
            } catch {}
        }
    }

    function _checkPriced(address token) internal {
        if (token == address(0)) return;

        // S23b: if the operator has routed this token through a TWAP, the
        // "priced" check chains to the route's quote token. A TWAP-routed
        // token whose quote token has a healthy aggregator IS considered
        // priced; if the quote leg is missing/stale, the QUOTE TOKEN is
        // recorded as unpriced (so monitoring sees the actual broken
        // dependency, not just the surface symptom on `token`).
        //
        // S23b-Polish-2 observability note: this check deliberately does
        // NOT probe `twapOracle.consult(...)` to detect TWAP-side health
        // failures (e.g., `NotPrimed`, `PairInactive`, pair deactivated).
        // Reasons:
        //   1. Gas: a try/consult inside `refreshUnpriced` would multiply
        //      its cost by every routed token AND incur an external call
        //      per-pair on each call, undermining the maxPairsToScan
        //      DoS-cap. `refreshUnpriced` is a permissionless monitoring
        //      tick that must stay cheap.
        //   2. Semantics: `NotPrimed` is a TRANSIENT condition that
        //      resolves at the next keeper `update`. Recording it as
        //      "unpriced" would generate noisy false-positives.
        //   3. Separation of concerns: TWAP-side health is observable
        //      directly via the TWAP's own surface — off-chain monitor
        //      should subscribe to:
        //        - `ZbxTwapOracle.PairDeactivated(pair)`         (operator deactivation)
        //        - `ZbxTwapOracle.PairRegistered(pair, period)`  (operator (re-)activation)
        //        - `ZbxTwapOracle.PeriodUpdated(pair, old, new)` (period reconfiguration)
        //        - `ZbxTwapOracle.PairCacheInvalidated(pair, old, new)`
        //                                                        (period INCREASE while cache is
        //                                                         currently primed → cache rebuild)
        //        - `ZbxTwapOracle.ObservationCommitted(pair, ...)` (heartbeat — absence
        //                                                        over a full `period` window
        //                                                        is the freshness signal)
        //      The TVL oracle's job is to surface AGGREGATOR-side
        //      failures it cannot otherwise externalise.
        //
        // The runtime fail-closed contract on `_safeUSD` (TWAP consult
        // revert → 0 contribution via try/catch) still holds, so a
        // transient `NotPrimed` cannot mis-report TVL upward — only
        // under-report, which is the deliberate fail-closed posture.
        TwapRoute memory r = twapRoute[token];
        if (r.enabled) {
            if (address(twapOracle) == address(0)) {
                _unpricedTokens.push(token);
                return;
            }
            _checkPricedAggregatorOnly(r.quoteToken);
            return;
        }

        _checkPricedAggregatorOnly(token);
    }

    function _checkPricedAggregatorOnly(address token) internal {
        if (token == address(0)) return;
        address feed = priceFeed[token];
        if (feed == address(0)) {
            _unpricedTokens.push(token);
            return;
        }
        try IAggregatorV3(feed).latestRoundData() returns (
            uint80, int256 price, uint256, uint256 updatedAt, uint80
        ) {
            if (price <= 0 || updatedAt + maxStaleness < block.timestamp) {
                _unpricedTokens.push(token);
            }
        } catch {
            _unpricedTokens.push(token);
        }
    }

    // ─── Admin ────────────────────────────────────────────────────────────

    function setPriceFeed(address token, address aggregator) external override onlyOwner {
        if (token == address(0)) revert ZeroAddress();
        priceFeed[token] = aggregator;   // address(0) = un-register
        emit PriceFeedSet(token, aggregator);
    }

    function setSource(Source src, address contractAddr) external override onlyOwner {
        _sources[uint8(src)] = contractAddr;   // address(0) = un-register
        emit SourceSet(src, contractAddr);
    }

    function setStabilityDepositToken(address token) external onlyOwner {
        // Token may be address(0) to disable the stability source.
        stabilityDepositToken = token;
    }

    // ─── S23b — TWAP alt-price-source admin ──────────────────────────────

    /// @inheritdoc IZbxTvlOracle
    function setTwapOracle(address oracle) external override onlyOwner {
        emit TwapOracleSet(address(twapOracle), oracle);
        twapOracle = IZbxTwapOracleLite(oracle);
    }

    /// @inheritdoc IZbxTvlOracle
    function setTwapRoute(
        address token,
        address pair,
        address quoteToken,
        bool    enabled
    )
        external
        override
        onlyOwner
    {
        if (token == address(0)) revert ZeroAddress();

        if (enabled) {
            if (pair == address(0))                 revert ZeroAddress();
            if (quoteToken == address(0))           revert ZeroAddress();
            if (quoteToken == token)                revert TwapPairTokenMismatch();
            // Quote MUST have an aggregator feed at config time. The path
            // remains fail-closed if the feed is later un-registered,
            // matching the existing missing/stale-feed policy.
            if (priceFeed[quoteToken] == address(0)) revert TwapQuoteUnpriced();
            // Hygiene: verify pair actually contains both legs. Catches
            // operator typos that would otherwise route a token's price
            // through an unrelated pool.
            address t0 = IZbxAMMPairLite(pair).token0();
            address t1 = IZbxAMMPairLite(pair).token1();
            bool tokenInPair = (t0 == token || t1 == token);
            bool quoteInPair = (t0 == quoteToken || t1 == quoteToken);
            if (!tokenInPair || !quoteInPair) revert TwapPairTokenMismatch();
        }

        twapRoute[token] = TwapRoute({
            pair:       pair,
            quoteToken: quoteToken,
            enabled:    enabled
        });
        emit TwapRouteSet(token, pair, quoteToken, enabled);
    }

    function setMaxStaleness(uint64 seconds_) external override onlyOwner {
        if (seconds_ == 0 || seconds_ > 7 days) revert InvalidStaleness();
        emit MaxStalenessSet(maxStaleness, seconds_);
        maxStaleness = seconds_;
    }

    function setMaxPairsToScan(uint16 cap) external override onlyOwner {
        if (cap == 0) revert InvalidPairCap();
        emit MaxPairsToScanSet(maxPairsToScan, cap);
        maxPairsToScan = cap;
    }

    function pause() external override onlyOwner {
        if (paused) revert AlreadyPaused();
        paused = true;
        emit Paused();
    }

    function unpause() external override onlyOwner {
        if (!paused) revert NotPaused();
        paused = false;
        emit Unpaused();
    }

    // ─── EIP-165 supportsInterface (S21) ───────────────────────────────────
    //
    // ZbxTvlOracle natively `is IZbxTvlOracle` (clean inheritance), so the
    // full interface claim is safe. Wallets / dashboards / governance UIs
    // can call this to discover that the contract exposes the canonical
    // TVL oracle surface (tvlBreakdown, totalValueLockedUSD, refresh,
    // pause/unpause, …) and the operator-only configuration setters.
    function supportsInterface(bytes4 interfaceId) external pure returns (bool) {
        return interfaceId == type(IZbxTvlOracle).interfaceId
            || interfaceId == 0x01ffc9a7;   // EIP-165 itself
    }

    // S18: `transferOwnership(newOwner)` and `renounceOwnership()` are now
    // inherited from `Ownable2Step` and use the two-step accept pattern.
    // The new owner MUST call `acceptOwnership()` from `newOwner` to take
    // over — `transferOwnership` only stages a `pendingOwner`.
}
