// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { IZRC20 } from "./IZRC20.sol";

/// @title IZRC20Mintable — Extension for tokens with controlled minting.
interface IZRC20Mintable is IZRC20 {
    event Mint(address indexed to, uint256 value);
    event MinterAdded(address indexed minter);
    event MinterRemoved(address indexed minter);
    event MintCapUpdated(uint256 oldCap, uint256 newCap);

    function mint(address to, uint256 value) external returns (bool);
    function mintCap() external view returns (uint256);
    function isMinter(address account) external view returns (bool);
    function addMinter(address account) external;
    function removeMinter(address account) external;
}