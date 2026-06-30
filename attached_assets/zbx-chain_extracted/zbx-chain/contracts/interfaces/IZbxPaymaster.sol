// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxPaymaster — Interface for ZbxPaymaster ERC-4337 gas sponsorship paymaster.
interface IZbxPaymaster {
    event GasSponsored(address indexed sender, uint256 gasCost);
    event DepositAdded(address indexed depositor, uint256 amount);
    event Withdrawn(address indexed recipient, uint256 amount);
    event PolicySet(address indexed account, bool sponsored);

    function validatePaymasterUserOp(bytes calldata userOp, bytes32 userOpHash, uint256 maxCost) external returns (bytes memory context, uint256 validationData);
    function postOp(uint8 mode, bytes calldata context, uint256 actualGasCost) external;
    function addDeposit() external payable;
    function withdrawTo(address payable recipient, uint256 amount) external;
    function setPolicy(address account, bool sponsored) external;
    function isSponsored(address account) external view returns (bool);
    function getDeposit() external view returns (uint256);
    function entryPoint() external view returns (address);
}
