// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IOwnable2Step — Interface for Ownable2Step two-step ownership transfer.
interface IOwnable2Step {
    event OwnershipTransferStarted(address indexed previousOwner, address indexed newOwner);
    event OwnershipTransferred(address indexed previousOwner, address indexed newOwner);

    function owner() external view returns (address);
    function pendingOwner() external view returns (address);
    function transferOwnership(address newOwner) external;
    function acceptOwnership() external;
    function renounceOwnership() external;
}
