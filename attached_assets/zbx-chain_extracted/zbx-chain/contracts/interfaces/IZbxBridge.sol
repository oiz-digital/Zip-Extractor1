// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxBridge — Interface for ZbxBridge cross-chain bridge contract.
interface IZbxBridge {
    event BridgeOutInitiated(address indexed token, address indexed sender, uint256 amount, bytes targetAddress, uint64 targetChain, uint256 indexed depositId);
    event BridgeInCompleted(address indexed token, address indexed recipient, uint256 amount, uint256 indexed depositId);
    event TokenWhitelisted(address indexed token, bool status);
    event RelayAdminUpdated(address indexed newAdmin);
    event GuardianUpdated(address indexed newGuardian);
    event BridgeInHourlyLimitUpdated(address indexed token, uint256 limit);
    event Paused(address indexed by);
    event Unpaused(address indexed by);

    function bridgeOut(address token, uint256 amount, bytes calldata targetAddress) external payable;
    function bridgeIn(address token, address recipient, uint256 amount, uint256 depositId, bytes[] calldata signatures) external;
    function addRelayer(address relayer) external;
    function removeRelayer(address relayer) external;
    function setTokenWhitelist(address token, bool status) external;
    function setHourlyLimit(address token, uint256 limit) external;
    function pause() external;
    function unpause() external;
    function paused() external view returns (bool);
    function isRelayer(address addr) external view returns (bool);
    function relayerCount() external view returns (uint256);
    function threshold() external view returns (uint256);
}
