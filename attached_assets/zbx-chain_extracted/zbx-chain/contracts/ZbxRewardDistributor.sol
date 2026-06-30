// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { Ownable2Step } from "./Ownable2Step.sol";

/// @title ZbxRewardDistributor — Staking rewards distributor.
/// @notice Distributes block rewards to validators and delegators.
///
/// @dev   Rewards come from two sources:
///          1. Block rewards (ZBX emission, decreasing over time)
///          2. Transaction fees (base fee is burned, priority fee to validators)
///
///        Distribution:
///          Validators: 80% of rewards (proportional to stake)
///          Delegators: 20% of rewards (proportional to delegated stake)
///          Treasury:   5% (protocol development fund)
///          Burn:       base fee burned (deflationary mechanism)
///
/// @custom:zbx-chain  Chain ID 8989

interface IZBX_Reward {
    function transfer(address to, uint256 amount) external returns (bool);
    function balanceOf(address) external view returns (uint256);
}

contract ZbxRewardDistributor is Ownable2Step {

    address public zbx;
    address public staking;
    address public treasury;
    // S18: `owner` inherited from Ownable2Step. Note this contract has a
    //      DUAL-role auth pattern: `staking || owner` for distribution.
    //      Only the pure-owner branch is migrated; the `staking` branch
    //      stays as an inline check.

    uint256 public constant VALIDATOR_SHARE = 80_00;  // 80% in bps
    uint256 public constant DELEGATOR_SHARE = 15_00;  // 15%
    uint256 public constant TREASURY_SHARE  =  5_00;  // 5%
    uint256 public constant BPS             = 100_00;

    /// validator → accumulated rewards
    mapping(address => uint256) public pendingRewards;
    mapping(address => uint256) public claimedRewards;

    uint256 public totalDistributed;
    uint256 public totalBurned;

    event RewardDistributed(uint256 blockHeight, uint256 totalReward);
    event RewardClaimed(address indexed validator, uint256 amount);
    event FeeBurned(uint256 amount);

    constructor(address zbx_, address staking_, address treasury_) Ownable2Step(msg.sender) {
        zbx      = zbx_;
        staking  = staking_;
        treasury = treasury_;
    }

    /// @notice Called by the protocol after each block to distribute rewards.
    function distributeBlockReward(
        uint256 blockHeight,
        address[] calldata validators,
        uint256[] calldata stakes,
        uint256 baseFee,
        uint256 priorityFees
    ) external {
        require(msg.sender == staking || msg.sender == owner, "Distributor: not authorised");
        require(validators.length == stakes.length, "Distributor: length mismatch");

        uint256 totalStake = 0;
        for (uint256 i = 0; i < stakes.length; i++) {
            totalStake += stakes[i];
        }
        require(totalStake > 0, "Distributor: no stake");

        // Block emission reward (from ZBX supply).
        uint256 blockEmission = _currentBlockReward(blockHeight);

        // Total to distribute = emission + priority fees.
        uint256 total = blockEmission + priorityFees;
        uint256 forValidators = total * VALIDATOR_SHARE / BPS;
        uint256 forTreasury   = total * TREASURY_SHARE  / BPS;

        // Distribute to validators proportional to stake.
        for (uint256 i = 0; i < validators.length; i++) {
            uint256 share = forValidators * stakes[i] / totalStake;
            pendingRewards[validators[i]] += share;
        }

        // Treasury share.
        pendingRewards[treasury] += forTreasury;

        // Burn the base fee.
        if (baseFee > 0) {
            totalBurned += baseFee;
            emit FeeBurned(baseFee);
        }

        totalDistributed += total;
        emit RewardDistributed(blockHeight, total);
    }

    /// @notice Validators call this to claim pending rewards.
    function claimRewards() external {
        uint256 amount = pendingRewards[msg.sender];
        require(amount > 0, "Distributor: no pending rewards");

        pendingRewards[msg.sender]  = 0;
        claimedRewards[msg.sender] += amount;

        require(IZBX_Reward(zbx).transfer(msg.sender, amount), "Distributor: transfer failed");
        emit RewardClaimed(msg.sender, amount);
    }

    /// @notice Current block emission reward (halves every 25M blocks).
    function _currentBlockReward(uint256 height) internal pure returns (uint256) {
        uint256 era    = height / 25_000_000;
        uint256 reward = 3e18;  // 3 ZBX initial block reward
        for (uint256 i = 0; i < era; i++) {
            reward = reward / 2;
        }
        return reward;
    }

    function getPendingReward(address validator) external view returns (uint256) {
        return pendingRewards[validator];
    }
}