// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxLending — Interface for ZbxLendingPool.
interface IZbxLending {
    struct Reserve {
        address token;
        uint256 totalDeposited;
        uint256 totalBorrowed;
        uint256 liquidityRate;
        uint256 borrowRate;
        uint256 utilizationRate;
        bool    active;
        bool    borrowEnabled;
    }

    struct UserDeposit {
        uint256 amount;
        uint256 shares;
        uint256 lastIndex;
    }

    event Deposit(address indexed token, address indexed user, uint256 amount);
    event Withdraw(address indexed token, address indexed user, uint256 amount);
    event Borrow(address indexed token, address indexed user, uint256 amount, address collateral);
    event Repay(address indexed token, address indexed user, uint256 amount);
    event Liquidated(
        address indexed token, address indexed collateral,
        address indexed borrower, uint256 debtRepaid, uint256 collateralSeized
    );

    function deposit(address token, uint256 amount)                        external;
    function withdraw(address token, uint256 amount)                       external;
    function borrow(address token, uint256 amount, address collateral)     external;
    function repay(address token, uint256 amount)                          external;
    function liquidate(address token, address borrower, address collateral) external;

    function getReserve(address token) external view returns (Reserve memory);
    function getUserBalance(address token, address user) external view returns (uint256);
    function getBorrowBalance(address token, address user) external view returns (uint256);
    function getUtilization(address token) external view returns (uint256);
}