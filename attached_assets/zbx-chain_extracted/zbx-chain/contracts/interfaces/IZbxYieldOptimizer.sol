// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxYieldOptimizer — Interface for ZbxYieldOptimizer yield aggregator vault.
interface IZbxYieldOptimizer {
    event Deposited(address indexed user, address indexed asset, uint256 amount, uint256 shares);
    event Withdrawn(address indexed user, address indexed asset, uint256 shares, uint256 amount);
    event Harvested(address indexed asset, uint256 yield_);
    event StrategyAdded(address indexed asset, address indexed strategy);

    function deposit(address asset, uint256 amount) external returns (uint256 shares);
    function withdraw(address asset, uint256 shares) external returns (uint256 amount);
    function harvest(address asset) external returns (uint256 yield_);
    function addStrategy(address asset, address strategy) external;
    function removeStrategy(address asset, address strategy) external;
    function totalAssets(address asset) external view returns (uint256);
    function pricePerShare(address asset) external view returns (uint256);
    function sharesOf(address user, address asset) external view returns (uint256);
    function getPendingYield(address asset) external view returns (uint256);
}
