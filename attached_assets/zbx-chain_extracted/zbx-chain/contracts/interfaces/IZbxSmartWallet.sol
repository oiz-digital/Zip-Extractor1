// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxSmartWallet — Interface for ZbxSmartWallet ERC-4337-compatible account abstraction wallet.
interface IZbxSmartWallet {
    struct UserOperation {
        address sender;
        uint256 nonce;
        bytes   initCode;
        bytes   callData;
        uint256 callGasLimit;
        uint256 verificationGasLimit;
        uint256 preVerificationGas;
        uint256 maxFeePerGas;
        uint256 maxPriorityFeePerGas;
        bytes   paymasterAndData;
        bytes   signature;
    }

    event ExecutionSuccess(bytes32 indexed txHash);
    event ExecutionFailure(bytes32 indexed txHash, string reason);
    event OwnerAdded(address indexed owner);
    event OwnerRemoved(address indexed owner);

    function execute(address to, uint256 value, bytes calldata data) external;
    function executeBatch(address[] calldata to, uint256[] calldata value, bytes[] calldata data) external;
    function validateUserOp(UserOperation calldata userOp, bytes32 userOpHash, uint256 missingAccountFunds) external returns (uint256 validationData);
    function addOwner(address owner) external;
    function removeOwner(address owner) external;
    function isOwner(address addr) external view returns (bool);
    function nonce() external view returns (uint256);
    function entryPoint() external view returns (address);
}
