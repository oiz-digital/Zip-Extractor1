// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxPerpetuals — Interface for ZbxPerpetuals on-chain perpetual futures.
interface IZbxPerpetuals {
    struct Position {
        address trader;
        bytes32 marketId;
        bool    isLong;
        uint256 size;
        uint256 margin;
        uint256 entryPrice;
        uint256 liquidationPrice;
        uint256 stopLoss;
        uint256 takeProfit;
        bool    liquidated;
    }

    event PositionOpened(bytes32 indexed positionId, address indexed trader, bytes32 marketId, bool isLong, uint256 size, uint256 margin);
    event PositionClosed(bytes32 indexed positionId, int256 pnl);
    event PositionLiquidated(bytes32 indexed positionId, address indexed liquidator, uint256 penalty);
    event MarketAdded(bytes32 indexed marketId, string symbol);

    error ZeroAmount();
    error LeverageTooHigh();
    error MarketNotFound();
    error PositionNotFound();
    error NotLiquidatable();
    error AlreadyLiquidated();

    function openPosition(bytes32 marketId, bool isLong, uint256 size, uint256 leverage, uint256 stopLoss, uint256 takeProfit) external payable returns (bytes32 positionId);
    function closePosition(bytes32 positionId) external returns (int256 pnl);
    function liquidate(bytes32 positionId) external;
    function adjustMargin(bytes32 positionId, uint256 additionalMargin) external payable;
    function getPosition(bytes32 positionId) external view returns (Position memory);
    function getMarketPrice(bytes32 marketId) external view returns (uint256);
    function isLiquidatable(bytes32 positionId) external view returns (bool);
    function addMarket(bytes32 marketId, string calldata symbol, uint256 maxLeverage, address oracle) external;
}
