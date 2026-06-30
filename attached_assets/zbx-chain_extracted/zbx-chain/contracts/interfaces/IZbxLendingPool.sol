// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxLendingPool — Interface for ZbxLendingPool over-collateralised lending protocol.
interface IZbxLendingPool {
    struct ReserveData {
        uint256 totalSupply;
        uint256 totalBorrows;
        uint256 supplyRate;
        uint256 borrowRate;
        uint256 lastUpdateBlock;
        address aToken;
        bool    active;
    }
    struct UserAccountData {
        uint256 totalCollateralUSD;
        uint256 totalDebtUSD;
        uint256 availableBorrowsUSD;
        uint256 healthFactor;
    }

    event Supply(address indexed asset, address indexed user, uint256 amount);
    event Withdraw(address indexed asset, address indexed user, uint256 amount);
    event Borrow(address indexed asset, address indexed user, uint256 amount);
    event Repay(address indexed asset, address indexed user, uint256 amount);
    event Liquidate(address indexed collateral, address indexed debt, address indexed user, uint256 debtCovered, address liquidator);
    event FlashLoan(address indexed receiver, address indexed asset, uint256 amount, uint256 fee);

    function supply(address asset, uint256 amount, address onBehalfOf) external;
    function withdraw(address asset, uint256 amount, address to) external returns (uint256);
    function borrow(address asset, uint256 amount, address onBehalfOf) external;
    function repay(address asset, uint256 amount, address onBehalfOf) external returns (uint256);
    function liquidate(address collateralAsset, address debtAsset, address user, uint256 debtToCover, bool receiveAToken) external;
    function flashLoan(address receiver, address asset, uint256 amount, bytes calldata params) external;
    function getReserveData(address asset) external view returns (ReserveData memory);
    function getUserAccountData(address user) external view returns (UserAccountData memory);
    function initReserve(address asset) external;
}
