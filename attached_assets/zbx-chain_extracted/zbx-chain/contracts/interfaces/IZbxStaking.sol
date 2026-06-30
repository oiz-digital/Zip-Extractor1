// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxStaking — Interface for ZbxStaking contract.
interface IZbxStaking {
    event Staked(address indexed validator, address indexed delegator, uint256 amount);
    event Unstaked(address indexed validator, address indexed delegator, uint256 amount);
    event RewardClaimed(address indexed delegator, uint256 amount);
    event ValidatorRegistered(address indexed validator, uint256 stake, uint16 commission);
    event ValidatorSlashed(address indexed validator, uint256 amount, string reason);

    struct ValidatorInfo {
        address addr;
        uint256 totalStake;
        uint256 selfStake;
        uint16  commissionBps;
        bool    active;
        bool    jailed;
        uint256 jailUntilBlock;
    }

    function stake(address validator) external payable;
    function unstake(address validator, uint256 amount) external;
    function claimRewards(address validator) external returns (uint256);
    function getValidatorInfo(address validator) external view returns (ValidatorInfo memory);
    function getDelegatorStake(address delegator, address validator) external view returns (uint256);
    function pendingRewards(address delegator, address validator) external view returns (uint256);
    function validatorCount() external view returns (uint256);
    function totalStaked() external view returns (uint256);
}