// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxMultisig — Interface for ZbxMultisig multi-signature wallet.
interface IZbxMultisig {
    event OwnerAdded(address indexed owner);
    event OwnerRemoved(address indexed owner);
    event RequirementChanged(uint256 newRequired);
    event TxSubmitted(uint256 indexed txId, address indexed from, address to, uint256 value);
    event TxConfirmed(uint256 indexed txId, address indexed owner);
    event TxRevoked(uint256 indexed txId, address indexed owner);
    event TxExecuted(uint256 indexed txId);
    event TxFailed(uint256 indexed txId);
    event Deposit(address indexed from, uint256 value);

    function submitTransaction(address to, uint256 value, bytes calldata data) external returns (uint256 txId);
    function confirmTransaction(uint256 txId) external;
    function revokeConfirmation(uint256 txId) external;
    function executeTransaction(uint256 txId) external;
    function addOwner(address owner) external;
    function removeOwner(address owner) external;
    function changeRequirement(uint256 newRequired) external;
    function isOwner(address addr) external view returns (bool);
    function getOwners() external view returns (address[] memory);
    function required() external view returns (uint256);
    function transactionCount() external view returns (uint256);
    function isConfirmed(uint256 txId) external view returns (bool);
    function getConfirmationCount(uint256 txId) external view returns (uint256);
}
