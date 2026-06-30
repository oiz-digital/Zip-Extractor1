// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

/// @title RewardPool — Protocol-level reward distribution for ZBX validators
/// @notice Stores block rewards + fee surplus. Validators and delegators claim
///         their share each epoch. Uses Synthetix reward_per_token algorithm.
/// @dev    Called by the staking precompile (0xC4) at each epoch boundary.

interface IZbxStaking {
    function totalStaked() external view returns (uint256);
    function stakedBy(address validator) external view returns (uint256);
}

contract RewardPool {

    // ── Constants ──────────────────────────────────────────────────────────

    /// Precision factor (1e18)
    uint256 constant PRECISION = 1e18;

    /// Epoch duration (approximately 1 day = 43200 blocks at 2s/block)
    uint256 constant EPOCH_BLOCKS = 43_200;

    /// Annual emission cap: 5% of total supply = 50M ZBX/year
    uint256 constant ANNUAL_EMISSION_CAP = 50_000_000 ether;

    /// Per-epoch emission cap = ANNUAL_EMISSION_CAP / 365
    uint256 constant EPOCH_EMISSION_CAP  = ANNUAL_EMISSION_CAP / 365;

    // ── State ──────────────────────────────────────────────────────────────

    address public owner;
    address public stakingContract;

    /// Accumulated reward per staked token (× PRECISION)
    uint256 public rewardPerToken;

    /// Total rewards ever distributed
    uint256 public totalRewardsEver;

    /// Current epoch number
    uint256 public currentEpoch;

    /// Block at which current epoch started
    uint256 public epochStartBlock;

    /// Per-validator: snapshot of rewardPerToken at last claim
    mapping(address => uint256) public validatorCheckpoint;

    /// Per-validator: accumulated but unclaimed rewards
    mapping(address => uint256) public accumulatedReward;

    /// Total ZBX in the reward pool
    uint256 public poolBalance;

    // ── Events ─────────────────────────────────────────────────────────────

    event EpochRewarded(uint256 indexed epoch, uint256 reward, uint256 rewardPerToken);
    event RewardClaimed(address indexed validator, uint256 amount);
    event RewardDeposited(address indexed from, uint256 amount);
    event PoolActivated(address stakingContract);

    // ── Errors ─────────────────────────────────────────────────────────────

    error OnlyOwner();
    error NothingToClaim();
    error EpochNotFinished(uint256 blocksRemaining);
    error RewardCapExceeded(uint256 requested, uint256 cap);

    // ── Constructor ────────────────────────────────────────────────────────

    constructor(address _stakingContract) {
        owner           = msg.sender;
        stakingContract = _stakingContract;
        epochStartBlock = block.number;
        emit PoolActivated(_stakingContract);
    }

    // ── Deposit ────────────────────────────────────────────────────────────

    /// @notice Deposit ZBX rewards (called by protocol, fee collector, treasury)
    receive() external payable {
        poolBalance += msg.value;
        emit RewardDeposited(msg.sender, msg.value);
    }

    // ── Epoch Settlement ───────────────────────────────────────────────────

    /// @notice Settle rewards for the completed epoch.
    ///         Can be called by anyone once EPOCH_BLOCKS have passed.
    /// @param  epochReward  ZBX to distribute this epoch (from pool balance)
    function settleEpoch(uint256 epochReward) external {
        uint256 blocksElapsed = block.number - epochStartBlock;
        if (blocksElapsed < EPOCH_BLOCKS) {
            revert EpochNotFinished(EPOCH_BLOCKS - blocksElapsed);
        }
        if (epochReward > EPOCH_EMISSION_CAP) {
            revert RewardCapExceeded(epochReward, EPOCH_EMISSION_CAP);
        }
        if (epochReward > poolBalance) {
            epochReward = poolBalance; // clamp to available
        }

        uint256 totalStaked = IZbxStaking(stakingContract).totalStaked();
        if (totalStaked == 0 || epochReward == 0) {
            _advanceEpoch();
            return;
        }

        // Update global accumulator (O(1) — no loops)
        rewardPerToken    += (epochReward * PRECISION) / totalStaked;
        totalRewardsEver  += epochReward;
        poolBalance       -= epochReward;

        emit EpochRewarded(currentEpoch, epochReward, rewardPerToken);
        _advanceEpoch();
    }

    // ── Claim ──────────────────────────────────────────────────────────────

    /// @notice Validator (or delegator via StakingPool) claims pending rewards.
    function claimReward() external returns (uint256 reward) {
        _settle(msg.sender);
        reward = accumulatedReward[msg.sender];
        if (reward == 0) revert NothingToClaim();

        accumulatedReward[msg.sender] = 0;
        (bool ok,) = msg.sender.call{value: reward}("");
        require(ok, "Transfer failed");

        emit RewardClaimed(msg.sender, reward);
    }

    // ── View ───────────────────────────────────────────────────────────────

    /// @notice Pending reward for an address.
    function pendingReward(address addr) external view returns (uint256) {
        uint256 staked = IZbxStaking(stakingContract).stakedBy(addr);
        uint256 delta  = rewardPerToken - validatorCheckpoint[addr];
        return accumulatedReward[addr] + (staked * delta / PRECISION);
    }

    /// @notice Estimated annual APR in basis points.
    function estimatedAprBps(uint256 epochReward) external view returns (uint256) {
        uint256 totalStaked = IZbxStaking(stakingContract).totalStaked();
        if (totalStaked == 0) return 0;
        uint256 annualReward = epochReward * 365;
        return (annualReward * 10_000) / totalStaked;
    }

    /// @notice Blocks until next epoch settles.
    function blocksUntilNextEpoch() external view returns (uint256) {
        uint256 elapsed = block.number - epochStartBlock;
        return elapsed >= EPOCH_BLOCKS ? 0 : EPOCH_BLOCKS - elapsed;
    }

    // ── Internal ───────────────────────────────────────────────────────────

    /// @dev Settle pending rewards for an address before state change.
    function _settle(address addr) internal {
        uint256 staked = IZbxStaking(stakingContract).stakedBy(addr);
        uint256 delta  = rewardPerToken - validatorCheckpoint[addr];
        accumulatedReward[addr]  += staked * delta / PRECISION;
        validatorCheckpoint[addr] = rewardPerToken;
    }

    function _advanceEpoch() internal {
        currentEpoch++;
        epochStartBlock = block.number;
    }
}