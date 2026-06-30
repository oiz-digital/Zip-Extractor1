// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxFlashLoan — Interface for ZbxFlashLoan.
interface IZbxFlashLoan {
    event FlashLoan(address indexed receiver, address indexed token, uint256 amount, uint256 fee);

    function flashLoan(
        address receiver, address token, uint256 amount, bytes calldata data
    ) external;

    function flashFee(address token, uint256 amount) external view returns (uint256);
    function maxFlashLoan(address token) external view returns (uint256);
}

/// @notice Implement this to receive a flash loan.
interface IFlashLoanReceiver {
    function onFlashLoan(
        address initiator, address token,
        uint256 amount, uint256 fee, bytes calldata data
    ) external returns (bytes32);
}