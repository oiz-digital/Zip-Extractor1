// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IRewardPool — Interface for RewardPool token reward distribution pool.
interface IRewardPool {
    event RewardAdded(uint256 amount);
    event RewardPaid(address indexed user, uint256 reward);
    event Staked(address indexed user, uint256 amount);
    event Withdrawn(address indexed user, uint256 amount);

    function stake(uint256 amount) external;
    function withdraw(uint256 amount) external;
    function getReward() external;
    function exit() external;
    function notifyRewardAmount(uint256 reward) external;
    function earned(address account) external view returns (uint256);
    function balanceOf(address account) external view returns (uint256);
    function totalSupply() external view returns (uint256);
    function rewardPerToken() external view returns (uint256);
    function rewardRate() external view returns (uint256);
    function periodFinish() external view returns (uint256);
}
