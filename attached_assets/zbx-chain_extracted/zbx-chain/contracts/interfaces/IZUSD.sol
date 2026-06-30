// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZUSD — Interface for the ZUSD stablecoin.
interface IZUSD {
    event Transfer(address indexed from, address indexed to, uint256 amount);
    event Approval(address indexed owner, address indexed spender, uint256 amount);
    event Mint(address indexed to, uint256 amount);
    event Burn(address indexed from, uint256 amount);

    function name()        external view returns (string memory);
    function symbol()      external view returns (string memory);
    function decimals()    external view returns (uint8);
    function totalSupply() external view returns (uint256);
    function balanceOf(address account) external view returns (uint256);
    function allowance(address owner, address spender) external view returns (uint256);

    function transfer(address to, uint256 amount)                         external returns (bool);
    function transferFrom(address from, address to, uint256 amount)       external returns (bool);
    function approve(address spender, uint256 amount)                     external returns (bool);

    /// @notice Mint ZUSD — only callable by ZusdVault.
    function mint(address to, uint256 amount) external;
    /// @notice Burn ZUSD — only callable by ZusdVault.
    function burn(address from, uint256 amount) external;

    function vault() external view returns (address);
}