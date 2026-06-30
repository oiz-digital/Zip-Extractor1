// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxTimelockController — Interface for ZbxTimelockController governance time-lock.
interface IZbxTimelockController {
    event CallScheduled(bytes32 indexed id, address target, uint256 value, bytes data, bytes32 predecessor, uint256 delay);
    event CallExecuted(bytes32 indexed id, address target, uint256 value, bytes data);
    event CallCancelled(bytes32 indexed id);
    event MinDelayChanged(uint256 oldDelay, uint256 newDelay);
    event RoleGranted(bytes32 indexed role, address indexed account, address indexed sender);
    event RoleRevoked(bytes32 indexed role, address indexed account, address indexed sender);

    function schedule(address target, uint256 value, bytes calldata data, bytes32 predecessor, bytes32 salt, uint256 delay) external;
    function execute(address target, uint256 value, bytes calldata data, bytes32 predecessor, bytes32 salt) external payable;
    function cancel(bytes32 id) external;
    function grantRole(bytes32 role, address account) external;
    function revokeRole(bytes32 role, address account) external;
    function hasRole(bytes32 role, address account) external view returns (bool);
    function isOperation(bytes32 id) external view returns (bool);
    function isOperationPending(bytes32 id) external view returns (bool);
    function isOperationReady(bytes32 id) external view returns (bool);
    function isOperationDone(bytes32 id) external view returns (bool);
    function getTimestamp(bytes32 id) external view returns (uint256);
    function getMinDelay() external view returns (uint256);
    function hashOperation(address target, uint256 value, bytes calldata data, bytes32 predecessor, bytes32 salt) external pure returns (bytes32);
}
