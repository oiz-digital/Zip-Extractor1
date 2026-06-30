// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxBundler — Interface for ZbxBundler ERC-4337 bundler contract.
interface IZbxBundler {
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

    event BundleSubmitted(address indexed bundler, uint256 opsCount, bytes32 indexed bundleId);
    event BundlerRegistered(address indexed bundler, uint256 stake);
    event BundlerSlashed(address indexed bundler, uint256 slashAmount, string reason);

    function registerBundler() external payable;
    function deregisterBundler() external;
    function submitBundle(UserOperation[] calldata ops, address payable beneficiary) external;
    function slash(address bundler, uint256 amount, string calldata reason) external;
    function isBundler(address addr) external view returns (bool);
    function bundlerStake(address bundler) external view returns (uint256);
    function minBundlerStake() external view returns (uint256);
}
