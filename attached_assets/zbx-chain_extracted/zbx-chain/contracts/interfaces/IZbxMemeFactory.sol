// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxMemeFactory — Interface for ZbxMemeFactory meme-token launcher with bonding curve.
interface IZbxMemeFactory {
    event TokenLaunched(address indexed token, address indexed creator, string name, string symbol, uint256 initialSupply);
    event TokenGraduated(address indexed token, address indexed pair, uint256 liquidityAdded);
    event Buy(address indexed token, address indexed buyer, uint256 zbxIn, uint256 tokensOut);
    event Sell(address indexed token, address indexed seller, uint256 tokensIn, uint256 zbxOut);

    function launch(string calldata name, string calldata symbol, uint256 totalSupply) external payable returns (address token);
    function buy(address token, uint256 minOut) external payable returns (uint256 tokensOut);
    function sell(address token, uint256 amount, uint256 minZbx) external returns (uint256 zbxOut);
    function graduate(address token) external;
    function getBuyPrice(address token, uint256 zbxIn) external view returns (uint256 tokensOut);
    function getSellPrice(address token, uint256 tokensIn) external view returns (uint256 zbxOut);
    function isGraduated(address token) external view returns (bool);
    function creatorOf(address token) external view returns (address);
    function graduationThreshold() external view returns (uint256);
}
