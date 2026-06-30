// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxAMMFactory — Interface for ZbxAMMFactory constant-product AMM factory.
interface IZbxAMMFactory {
    event PairCreated(address indexed token0, address indexed token1, address pair, uint256 pairIndex);

    error IdenticalAddresses();
    error ZeroAddress();
    error PairExists();
    error PairCreateFailed();

    function createPair(address tokenA, address tokenB) external returns (address pair);
    function getPair(address tokenA, address tokenB) external view returns (address pair);
    function allPairs(uint256 index) external view returns (address pair);
    function allPairsLength() external view returns (uint256);
    function predictPair(address tokenA, address tokenB) external view returns (address);
    function getInitHash() external pure returns (bytes32);
    function feeTo() external view returns (address);
    function setFeeTo(address) external;
}
