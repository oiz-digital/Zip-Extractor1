// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxEntryPoint — Interface for ZbxEntryPoint (ERC-4337).
interface IZbxEntryPoint {
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

    function handleOps(UserOperation[] calldata ops, address payable beneficiary) external;
    function getUserOpHash(UserOperation calldata op) external view returns (bytes32);
    function depositTo(address account) external payable;
    function withdrawTo(address payable to, uint256 amount) external;
    function balanceOf(address account) external view returns (uint256);
}