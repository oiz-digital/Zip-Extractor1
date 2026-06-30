// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

/// @title ZbxOracleConsumer — Example of using ZBX price feeds in a contract
/// @notice Shows how to read ZBX/USD, ETH/USD, BTC/USD prices.
///         Identical interface to Chainlink — just use ZbxAggregatorV3 address.

interface AggregatorV3Interface {
    function latestRoundData() external view returns (
        uint80 roundId, int256 answer,
        uint256 startedAt, uint256 updatedAt, uint80 answeredInRound
    );
    function decimals() external view returns (uint8);
}

contract ZbxOracleConsumer {

    // Feed addresses (deployed on ZBX chain).
    // NOTE: previous values were 41 hex chars and would fail to compile —
    // padded to a valid 40-hex-char (20-byte) address. Replace at deploy time
    // with the real registry addresses.
    address public constant ZBX_USD_FEED  = 0xfeED0001000000000000000000000000000000F1; // ZBX/USD
    address public constant ZUSD_USD_FEED = 0xfeed0002000000000000000000000000000000F2; // ZUSD/USD
    address public constant ETH_USD_FEED  = 0xFEEd0003000000000000000000000000000000F3; // ETH/USD
    address public constant BTC_USD_FEED  = 0xfEeD0004000000000000000000000000000000F4; // BTC/USD

    /// @notice Maximum age of a Chainlink-style answer before consumers should
    ///         treat it as stale. 1 hour matches Chainlink heartbeat for major
    ///         pairs; tighten per-feed in production.
    uint256 public constant MAX_PRICE_STALENESS = 1 hours;

    /// @dev Reads `latestRoundData` and asserts the answer is positive and
    ///      not stale. All consumers must route through this helper.
    function _readFresh(address feed) private view returns (int256 answer) {
        (, int256 a,, uint256 updatedAt,) =
            AggregatorV3Interface(feed).latestRoundData();
        require(a > 0, "Oracle: invalid price");
        require(updatedAt > 0, "Oracle: round not complete");
        require(block.timestamp - updatedAt <= MAX_PRICE_STALENESS, "Oracle: stale price");
        return a;
    }

    /// @notice Get the current ZBX/USD price in USD (8 decimals).
    /// @return price  Current price (e.g. 2_50000000 = $2.50)
    /// @return age    Seconds since last update
    function getZbxPrice() external view returns (int256 price, uint256 age) {
        (, int256 answer,, uint256 updatedAt,) =
            AggregatorV3Interface(ZBX_USD_FEED).latestRoundData();
        require(answer > 0, "Oracle: invalid price");
        require(updatedAt > 0, "Oracle: round not complete");
        require(block.timestamp - updatedAt <= MAX_PRICE_STALENESS, "Oracle: stale price");
        return (answer, block.timestamp - updatedAt);
    }

    /// @notice Get the ZUSD/USD price (should always be ~1.00).
    function getZusdPeg() external view returns (int256 price, bool isPegged) {
        int256 answer = _readFresh(ZUSD_USD_FEED);
        // isPegged = within 0.5% of $1.00
        int256 deviation = answer - 1_00000000;
        isPegged = (deviation < 500000 && deviation > -500000);
        return (answer, isPegged);
    }

    /// @notice Convert a USD amount to ZBX.
    /// @param usdAmount Amount in USD (8 decimals, e.g. 100_00000000 = $100)
    /// @return zbxAmount Equivalent ZBX (8 decimals)
    function usdToZbx(int256 usdAmount) external view returns (int256 zbxAmount) {
        int256 zbxPrice = _readFresh(ZBX_USD_FEED);
        // zbxAmount = usdAmount / zbxPrice × 10^8 (to maintain decimals)
        zbxAmount = (usdAmount * 1e8) / zbxPrice;
    }

    /// @notice Check if collateral value covers a loan (for ZUSD minting).
    /// @param collateralZbx Amount of ZBX collateral (wei)
    /// @param loanZusd      Requested ZUSD loan (18 decimals)
    /// @param ratio         Required collateral ratio (e.g. 150 = 150%)
    function isCollateralized(
        uint256 collateralZbx,
        uint256 loanZusd,
        uint256 ratio
    ) external view returns (bool) {
        int256 zbxPrice = _readFresh(ZBX_USD_FEED);

        // collateral value in USD (8 decimals)
        uint256 collateralUsd = collateralZbx * uint256(zbxPrice) / 1e18;
        // loan value in USD (8 decimals, ZUSD is 1:1)
        uint256 loanUsd = loanZusd / 1e10;

        // collateralUsd ≥ loanUsd × ratio / 100
        return collateralUsd * 100 >= loanUsd * ratio;
    }
}