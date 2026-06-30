// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxFaucet — Interface for ZbxFaucet testnet token faucet.
interface IZbxFaucet {
    event Dispensed(address indexed to, uint256 amount);
    event Funded(address indexed from, uint256 value);
    event Paused(bool status);

    function request() external;
    function cooldownRemaining(address user) external view returns (uint256);
    function setPaused(bool status) external;
    function withdraw() external;
    function drip() external view returns (uint256);
    function cooldown() external view returns (uint256);
    function paused() external view returns (bool);
}
