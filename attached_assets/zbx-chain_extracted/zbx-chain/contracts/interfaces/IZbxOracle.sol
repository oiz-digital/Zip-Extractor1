// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxOracle — Interface for ZbxOracle price feed.
interface IZbxOracle {
    event PriceUpdated(address indexed asset, uint256 price, uint256 timestamp);
    event FeederAdded(address indexed feeder);
    event FeederRemoved(address indexed feeder);

    /// @notice Get latest price of `asset` in USD (8 decimals).
    function getPrice(address asset) external view returns (uint256);

    /// @notice Get price with timestamp (for staleness checks).
    function getPriceWithTimestamp(address asset)
        external view returns (uint256 price, uint256 updatedAt);

    /// @notice Get TWAP price over `period` seconds.
    function getTwapPrice(address asset, uint256 period) external view returns (uint256);

    /// @notice Whether price is fresh (within 1 hour).
    function isFresh(address asset) external view returns (bool);
}