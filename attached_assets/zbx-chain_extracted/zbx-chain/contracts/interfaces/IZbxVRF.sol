// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxVRF — Interface for ZbxVRF on-chain Verifiable Random Function.
interface IZbxVRF {
    event RandomnessRequested(bytes32 indexed requestId, address indexed requester, uint256 seed);
    event RandomnessFulfilled(bytes32 indexed requestId, uint256 randomWord);

    function requestRandomness(uint256 seed) external returns (bytes32 requestId);
    function fulfillRandomness(bytes32 requestId, uint256 randomWord, bytes calldata proof) external;
    function getResult(bytes32 requestId) external view returns (bool fulfilled, uint256 randomWord);
    function isPending(bytes32 requestId) external view returns (bool);
    function setOracle(address oracle) external;
    function oracle() external view returns (address);
}
