// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { IZRC20 } from "./IZRC20.sol";

/// @title IZRC20Burnable — Extension for tokens that can be permanently destroyed.
interface IZRC20Burnable is IZRC20 {
    event Burn(address indexed from, uint256 value);

    function burn(uint256 value) external returns (bool);
    function burnFrom(address from, uint256 value) external returns (bool);
    function totalBurned() external view returns (uint256);
}