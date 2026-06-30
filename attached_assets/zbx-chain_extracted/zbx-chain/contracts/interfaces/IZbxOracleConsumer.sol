// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxOracleConsumer — Interface for ZbxOracleConsumer generic oracle data consumer.
interface IZbxOracleConsumer {
    event PriceUpdated(bytes32 indexed feedId, uint256 price, uint256 timestamp);
    event FeedRegistered(bytes32 indexed feedId, address oracle);

    function registerFeed(bytes32 feedId, address oracle) external;
    function updatePrice(bytes32 feedId) external;
    function getLatestPrice(bytes32 feedId) external view returns (uint256 price, uint256 updatedAt);
    function getTwapPrice(bytes32 feedId, uint256 windowSeconds) external view returns (uint256 twapPrice);
    function isFeedActive(bytes32 feedId) external view returns (bool);
    function staleness(bytes32 feedId) external view returns (uint256 secondsSinceUpdate);
}
