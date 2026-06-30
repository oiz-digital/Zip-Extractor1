// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxTvlOracle — Standard interface for the Zebvix Chain TVL aggregator.
/// @notice Aggregates Total Value Locked across every protocol-level liquidity
///         source on Zebvix Chain (Chain ID 8989 mainnet / 8990 testnet+devnet)
///         and reports the result in USD with 18-decimal precision.
///
/// @dev Sources currently aggregated:
///   - AMM          — sum of every ZbxAMM pair's reserves × USD price.
///   - LENDING      — sum of (real totalSupplied − real totalBorrowed) per
///                    reserve × USD price; "real" applies the liquidityIndex
///                    / borrowIndex unscaling to the stored scaled values.
///   - STABILITY    — ZusdStabilityPool.totalDeposits() (ZUSD priced).
///   - STAKING      — ZRC20Staking.totalStaked × stake-token USD price.
///   - REWARD       — scaffolded (returns 0 until configured).
///   - BRIDGE_VAULT — scaffolded (returns 0 until configured).
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:standard   ZEP-007-TVL-ORACLE v1
/// @custom:audits     Pending — not for mainnet use without audit.
interface IZbxTvlOracle {

    enum Source { AMM, LENDING, STABILITY, STAKING, REWARD, BRIDGE_VAULT }

    struct TvlBreakdown {
        uint256 amm;
        uint256 lending;
        uint256 stability;
        uint256 staking;
        uint256 reward;
        uint256 bridgeVault;
        uint256 total;
        uint256 timestamp;
    }

    // ─── Aggregated views (USD, 18 decimals) ───────────────────────────────

    /// @notice Sum of every source's USD value at the current block.
    function totalValueLockedUSD() external view returns (uint256);

    /// @notice TVL contribution of a single source (USD, 18 decimals).
    function tvlBySource(Source src) external view returns (uint256);

    /// @notice TVL of a specific token across every source (USD, 18 decimals).
    /// @dev Useful for "how much DAI / ZUSD / WBTC is locked across the chain".
    function tvlByToken(address token) external view returns (uint256);

    /// @notice Single-call snapshot — preferred by off-chain indexers to
    ///         avoid storage-state drift between per-source RPC calls.
    function tvlBreakdown() external view returns (TvlBreakdown memory);

    // ─── Per-source views ──────────────────────────────────────────────────

    function tvlAMM()         external view returns (uint256);
    function tvlLending()     external view returns (uint256);
    function tvlStability()   external view returns (uint256);
    function tvlStaking()     external view returns (uint256);
    function tvlReward()      external view returns (uint256);
    function tvlBridgeVault() external view returns (uint256);

    // ─── Configuration views ───────────────────────────────────────────────

    function priceFeed(address token)  external view returns (address);
    function source(Source src)        external view returns (address);
    function maxStaleness()            external view returns (uint64);
    function maxPairsToScan()          external view returns (uint16);
    function paused()                  external view returns (bool);

    /// @notice S23b — Address of the TWAP oracle (ZEP-008). `address(0)`
    ///         when no oracle is wired (TWAP routing disabled chain-wide).
    function twapOracle() external view returns (address);

    /// @notice S23b — Per-token TWAP routing config. When `enabled`,
    ///         `_safeUSD` resolves via `twapOracle.consult(pair, token, amt)`
    ///         to obtain the amount in `quoteToken`, then resolves
    ///         `quoteToken` via its aggregator feed. The setter enforces
    ///         that `quoteToken` has an aggregator feed AND that `pair`
    ///         contains both `token` and `quoteToken`.
    function twapRoute(address token)
        external view returns (address pair, address quoteToken, bool enabled);

    /// @notice Set of tokens encountered during the most recent
    ///         `refreshUnpriced()` call that lacked a working price feed.
    ///         Off-chain monitoring should call `refreshUnpriced()` then
    ///         read this getter; alert if non-empty.
    function unpricedTokens() external view returns (address[] memory);

    /// @notice Mutator twin of the `tvl*` views. Walks every configured
    ///         source and records tokens lacking a working price feed
    ///         (missing aggregator, stale, negative, or revert) into
    ///         `_unpricedTokens`. Cleared at the start of each call.
    /// @dev    Called by off-chain monitoring (e.g. once per minute).
    ///         No return value — call `unpricedTokens()` afterward.
    ///         Emits `PairScanTruncated` if AMM pair count exceeds cap.
    function refreshUnpriced() external;

    /// @notice AMM scan visibility — operators should monitor `truncated`
    ///         to know when to raise `maxPairsToScan`. Truncation makes
    ///         `tvlAMM()` a *lower bound* on the true AMM TVL.
    function pairScanStats()
        external
        view
        returns (uint256 totalPairs, uint256 scanned, bool truncated);

    // ─── Admin (owner-only) ────────────────────────────────────────────────

    function setPriceFeed(address token, address aggregator) external;
    function setSource(Source src, address contractAddr) external;
    function setMaxStaleness(uint64 seconds_) external;
    function setMaxPairsToScan(uint16 cap) external;
    function pause() external;
    function unpause() external;

    /// @notice S23b — Wire (or re-wire) the TWAP oracle. Pass `address(0)`
    ///         to disable TWAP routing chain-wide (all `twapRoute[*]`
    ///         entries fail-close to ZERO contribution).
    function setTwapOracle(address oracle) external;

    /// @notice S23b — Set or clear a per-token TWAP route. When
    ///         `enabled = false`, `pair` and `quoteToken` are ignored and
    ///         `_safeUSD` reverts to the legacy aggregator-only path.
    ///         When `enabled = true`, all of these MUST hold:
    ///           - `quoteToken` has an aggregator `priceFeed` registered
    ///           - `pair` contains both `token` and `quoteToken`
    ///           - `quoteToken != token`
    ///         Otherwise reverts `TwapQuoteUnpriced` or `TwapPairTokenMismatch`.
    function setTwapRoute(
        address token,
        address pair,
        address quoteToken,
        bool    enabled
    ) external;

    // ─── Events ────────────────────────────────────────────────────────────

    event PriceFeedSet(address indexed token, address indexed aggregator);
    event SourceSet(Source indexed src, address indexed contractAddr);
    event MaxStalenessSet(uint64 oldValue, uint64 newValue);
    event MaxPairsToScanSet(uint16 oldValue, uint16 newValue);
    event Paused();
    event Unpaused();

    /// @notice Emitted from `refreshUnpriced()` whenever the AMM factory
    ///         exposes more pairs than `maxPairsToScan`. The reported
    ///         `tvlAMM()` for that block is a lower bound on the truth.
    event PairScanTruncated(uint256 totalPairs, uint256 scanned);

    /// @notice S23b — Emitted when `setTwapOracle` rewires the alt-price
    ///         source. `oldOracle == address(0)` on first wiring.
    event TwapOracleSet(address indexed oldOracle, address indexed newOracle);

    /// @notice S23b — Emitted when `setTwapRoute` adds, updates, or
    ///         disables a per-token TWAP route.
    event TwapRouteSet(
        address indexed token,
        address indexed pair,
        address indexed quoteToken,
        bool    enabled
    );

    // ─── Errors ────────────────────────────────────────────────────────────

    // S18: `error NotOwner()` is now provided by `Ownable2Step` base in
    //       the implementation. Removed from this interface to avoid a
    //       duplicate declaration when a contract inherits both.
    error ZeroAddress();
    error InvalidStaleness();
    error InvalidPairCap();
    error AlreadyPaused();
    error NotPaused();
    error PausedQuery();
    error UnknownSource();

    /// @notice S23b — `setTwapRoute(enabled=true)` rejected because
    ///         `quoteToken` does not have an aggregator `priceFeed`
    ///         registered. Configure the quote feed first, then route.
    error TwapQuoteUnpriced();

    /// @notice S23b — `setTwapRoute(enabled=true)` rejected because
    ///         `pair` does not contain both `token` and `quoteToken`,
    ///         or because `quoteToken == token` (degenerate self-quote).
    error TwapPairTokenMismatch();
}
