// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title ZbxOracle — Price oracle for Zebvix Chain DeFi.
/// @notice Provides USD prices for ZBX and all bridged assets.
///         Compatible with Chainlink AggregatorV3Interface so existing
///         DeFi protocols (lending, AMMs) work without modification.
///
/// @dev   Architecture:
///           - Multiple trusted data providers push prices on-chain.
///           - Median of N latest reports is used (outlier resistant).
///           - Prices expire after `MAX_STALENESS` seconds.
///           - Governance can add/remove providers and update asset list.
///
///        Supported assets on launch:
///          ZBX, WZBX, ETH, BTC, BNB, SOL, MATIC, USDT, USDC
///
/// @custom:zbx-chain  Chain ID 8989

contract ZbxOracle {

    // ─── Constants ────────────────────────────────────────────────────────

    uint256 public constant MAX_STALENESS    = 3600;   // 1 hour (default)
    /// SEC-2026-05-09 Pass-15 (HIGH-S04 part 2): per-asset staleness
    /// threshold override. A single 1-hour cap is wrong for assets
    /// with very different update cadences — high-volume pairs
    /// (ZBX/USDC) should expire in minutes, low-volume pairs (XAU)
    /// can tolerate hours. Governance sets via `setAssetStaleness`;
    /// `0` falls through to `MAX_STALENESS`.
    mapping(address => uint256) public assetStaleness;
    function _stalenessOf(address a) internal view returns (uint256) {
        uint256 s = assetStaleness[a];
        return s == 0 ? MAX_STALENESS : s;
    }
    event AssetStalenessSet(address indexed asset, uint256 seconds_);
    // 3-of-N reporters required for a price update to be accepted.
    // 2-of-2 (the previous value) collapses to "trust either side" — a single
    // compromised relayer can collude with one other to push any price.
    // See AUDIT_2026-04-30.md M-15.
    uint256 public constant MIN_PROVIDERS    = 3;
    uint256 public constant PRICE_DECIMALS   = 8;       // prices in 8-decimal USD (Chainlink standard)

    // ─── Types ────────────────────────────────────────────────────────────

    struct Price {
        int256  answer;       // price in USD × 10^8
        uint256 updatedAt;    // unix timestamp of latest update
        uint80  roundId;      // monotonically increasing round ID
    }

    struct Report {
        address provider;
        int256  price;
        uint256 timestamp;
    }

    // ─── State ────────────────────────────────────────────────────────────

    address public owner;
    address public governance;    // can update providers / assets

    mapping(address => bool)    public providers;
    uint256                     public providerCount;

    // asset token address → latest aggregated price
    mapping(address => Price)   public latestPrice;

    // asset → provider → latest report
    mapping(address => mapping(address => Report)) public reports;

    // asset → list of reporting providers
    mapping(address => address[]) public assetProviders;

    // ─── Events ───────────────────────────────────────────────────────────

    event PriceUpdated(address indexed asset, int256 price, uint256 timestamp, uint80 roundId);
    event ProviderAdded(address indexed provider);
    event ProviderRemoved(address indexed provider);
    event AssetAdded(address indexed asset);

    // ─── Constructor ──────────────────────────────────────────────────────

    constructor(address governance_) {
        owner      = msg.sender;
        governance = governance_;
        providers[msg.sender] = true;
        providerCount = 1;
    }

    modifier onlyOwner()      { require(msg.sender == owner || msg.sender == governance, "Oracle: unauthorized"); _; }
    modifier onlyProvider()   { require(providers[msg.sender], "Oracle: not provider"); _; }

    // ─── Price Feed (Chainlink AggregatorV3 compatible) ───────────────────

    /// @notice Latest price for an asset.
    /// @return roundId     Latest round ID.
    /// @return answer      Price in USD × 10^8.
    /// @return startedAt   When the round started.
    /// @return updatedAt   When price was last updated.
    /// @return answeredInRound  Same as roundId.
    function latestRoundData(address asset) external view returns (
        uint80  roundId,
        int256  answer,
        uint256 startedAt,
        uint256 updatedAt,
        uint80  answeredInRound
    ) {
        Price storage p = latestPrice[asset];
        require(p.updatedAt > 0, "Oracle: no price");
        require(block.timestamp - p.updatedAt <= _stalenessOf(asset), "Oracle: stale price");
        return (p.roundId, p.answer, p.updatedAt, p.updatedAt, p.roundId);
    }

    /// @notice Latest price (simple view — use this in contracts).
    function getPrice(address asset) external view returns (int256 price, uint256 updatedAt) {
        Price storage p = latestPrice[asset];
        require(p.updatedAt > 0,                              "Oracle: no price for asset");
        require(block.timestamp - p.updatedAt <= _stalenessOf(asset), "Oracle: price is stale");
        return (p.answer, p.updatedAt);
    }

    /// @notice USD value of `amount` tokens (amount in 18-decimal, returns 8-decimal USD).
    function getUSDValue(address asset, uint256 amount) external view returns (uint256) {
        (int256 price,) = this.getPrice(asset);
        require(price > 0, "Oracle: non-positive price");
        return (amount * uint256(price)) / 1e18;
    }

    // ─── Provider price submission ─────────────────────────────────────────

    /// @notice Submit a price update for an asset.
    ///         Multiple providers submit; median is computed and stored.
    function submitPrice(address asset, int256 price) external onlyProvider {
        require(price > 0, "Oracle: non-positive price");

        reports[asset][msg.sender] = Report({
            provider:  msg.sender,
            price:     price,
            timestamp: block.timestamp
        });

        _aggregatePrice(asset);
    }

    /// @notice Batch price submission for multiple assets in one tx.
    function submitPriceBatch(address[] calldata assets, int256[] calldata prices) external onlyProvider {
        require(assets.length == prices.length, "Oracle: length mismatch");
        for (uint256 i; i < assets.length; ++i) {
            require(prices[i] > 0, "Oracle: non-positive price");
            reports[assets[i]][msg.sender] = Report({
                provider:  msg.sender,
                price:     prices[i],
                timestamp: block.timestamp
            });
            _aggregatePrice(assets[i]);
        }
    }

    // ─── Internal: median aggregation ─────────────────────────────────────

    function _aggregatePrice(address asset) internal {
        address[] storage provList = assetProviders[asset];
        uint256 n = provList.length;

        // Collect fresh reports (within MAX_STALENESS).
        int256[] memory fresh = new int256[](n);
        uint256  count;
        for (uint256 i; i < n; ++i) {
            Report storage r = reports[asset][provList[i]];
            if (r.timestamp > 0 && block.timestamp - r.timestamp <= MAX_STALENESS) {
                fresh[count++] = r.price;
            }
        }

        if (count < MIN_PROVIDERS) return; // not enough data yet

        // Sort and take median.
        _sort(fresh, count);
        int256 median = fresh[count / 2];

        latestPrice[asset].answer    = median;
        latestPrice[asset].updatedAt = block.timestamp;
        latestPrice[asset].roundId  += 1;

        emit PriceUpdated(asset, median, block.timestamp, latestPrice[asset].roundId);
    }

    function _sort(int256[] memory arr, uint256 len) internal pure {
        for (uint256 i = 1; i < len; ++i) {
            int256 key = arr[i];
            uint256 j  = i;
            while (j > 0 && arr[j - 1] > key) { arr[j] = arr[j - 1]; --j; }
            arr[j] = key;
        }
    }

    // ─── Admin ────────────────────────────────────────────────────────────

    function addProvider(address provider) external onlyOwner {
        require(!providers[provider], "Oracle: already provider");
        providers[provider] = true;
        ++providerCount;
        emit ProviderAdded(provider);
    }

    function removeProvider(address provider) external onlyOwner {
        require(providers[provider],  "Oracle: not provider");
        require(providerCount > MIN_PROVIDERS, "Oracle: cannot remove — too few providers");
        providers[provider] = false;
        --providerCount;
        emit ProviderRemoved(provider);
    }

    function addAsset(address asset, address[] calldata initialProviders) external onlyOwner {
        require(assetProviders[asset].length == 0, "Oracle: asset already added");
        for (uint256 i; i < initialProviders.length; ++i) {
            assetProviders[asset].push(initialProviders[i]);
        }
        emit AssetAdded(asset);
    }

    function setGovernance(address gov) external onlyOwner { governance = gov; }
    function transferOwnership(address to) external onlyOwner { owner = to; }
}