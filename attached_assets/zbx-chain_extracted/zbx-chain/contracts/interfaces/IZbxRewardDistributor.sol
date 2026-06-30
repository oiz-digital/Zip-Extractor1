// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxRewardDistributor — Interface for ZbxRewardDistributor block-reward split contract.
interface IZbxRewardDistributor {
    event RewardDistributed(uint256 blockHeight, uint256 totalReward);
    event RewardClaimed(address indexed validator, uint256 amount);
    event FeeBurned(uint256 amount);

    function distributeBlockReward(address[] calldata validators, uint256[] calldata weights, uint256 blockHeight) external payable;
    function claimRewards() external;
    function getPendingReward(address validator) external view returns (uint256);
    function totalDistributed() external view returns (uint256);
}
