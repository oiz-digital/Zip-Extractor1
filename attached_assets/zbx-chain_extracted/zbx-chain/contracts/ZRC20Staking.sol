// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { IZRC20 }          from "./interfaces/IZRC20.sol";
import { ReentrancyGuard } from "./libraries/ReentrancyGuard.sol";

/// @title ZRC20Staking — Single-token staking vault for any ZRC-20 token.
/// @notice Users stake a ZRC-20 token and earn rewards in the same or another ZRC-20 token.
///         Rewards accumulate per second based on a configurable reward rate.
///
/// @dev  Reward calculation:
///         rewardPerToken += rewardRate * dt / totalStaked
///         userReward     += staked * (rewardPerToken - userRewardDebt)
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:standard   ZRC-20 v1.0

contract ZRC20Staking is ReentrancyGuard {

    // ─── Events ───────────────────────────────────────────────────────────

    event Staked(address indexed user, uint256 amount);
    event Withdrawn(address indexed user, uint256 amount);
    event RewardClaimed(address indexed user, uint256 reward);
    event RewardRateUpdated(uint256 newRate);
    event EmergencyWithdraw(address indexed user, uint256 amount);
    event OwnershipTransferStarted(address indexed prev, address indexed next);
    event OwnershipTransferred(address indexed prev, address indexed next);

    // ─── State ────────────────────────────────────────────────────────────

    IZRC20 public immutable stakingToken;
    IZRC20 public immutable rewardToken;
    address public owner;
    /// @notice Pending new owner — must call `acceptOwnership()` to complete the transfer.
    address public pendingOwner;

    uint256 public rewardRate;          // reward tokens per second (18-decimal)
    uint256 public rewardPerTokenStored;
    uint256 public lastUpdateTime;
    uint256 public totalStaked;

    mapping(address => uint256) public stakedAmount;
    mapping(address => uint256) public userRewardPerTokenPaid;
    mapping(address => uint256) public rewards;

    // ─── Constructor ──────────────────────────────────────────────────────

    constructor(address stakingToken_, address rewardToken_, uint256 rewardRate_) {
        stakingToken = IZRC20(stakingToken_);
        rewardToken  = IZRC20(rewardToken_);
        rewardRate   = rewardRate_;
        lastUpdateTime = block.timestamp;
        owner = msg.sender;
    }

    // ─── Modifiers ────────────────────────────────────────────────────────

    modifier update(address account) {
        rewardPerTokenStored = _rewardPerToken();
        lastUpdateTime = block.timestamp;
        if (account != address(0)) {
            rewards[account] = earned(account);
            userRewardPerTokenPaid[account] = rewardPerTokenStored;
        }
        _;
    }

    modifier onlyOwner() { require(msg.sender == owner, "Staking: not owner"); _; }

    // ─── User Actions ─────────────────────────────────────────────────────

    function stake(uint256 amount) external nonReentrant update(msg.sender) {
        require(amount > 0, "Staking: zero amount");
        stakedAmount[msg.sender] += amount;
        totalStaked              += amount;
        stakingToken.transferFrom(msg.sender, address(this), amount);
        emit Staked(msg.sender, amount);
    }

    function withdraw(uint256 amount) external nonReentrant update(msg.sender) {
        _withdraw(msg.sender, amount);
    }

    function claimReward() external nonReentrant update(msg.sender) {
        _claimReward(msg.sender);
    }

    /// @notice Withdraw entire stake AND claim accrued rewards in one tx.
    /// @dev    Single nonReentrant guard at this entry point — the inner
    ///         helpers `_withdraw` / `_claimReward` are unguarded internals,
    ///         so we don't trip the same-contract reentrancy revert. The
    ///         `update` modifier runs once (vs twice if we naively called
    ///         the externals via `this.`), saving ~2 SLOADs.
    function exit() external nonReentrant update(msg.sender) {
        _withdraw(msg.sender, stakedAmount[msg.sender]);
        _claimReward(msg.sender);
    }

    // ─── Internal helpers (no guard — entry points wrap with nonReentrant) ─

    function _withdraw(address user, uint256 amount) internal {
        require(amount > 0,                       "Staking: zero amount");
        require(stakedAmount[user] >= amount,     "Staking: insufficient stake");
        stakedAmount[user] -= amount;
        totalStaked        -= amount;
        // Checks-effects-interactions: state changes above precede external call.
        stakingToken.transfer(user, amount);
        emit Withdrawn(user, amount);
    }

    function _claimReward(address user) internal {
        uint256 reward = rewards[user];
        if (reward > 0) {
            rewards[user] = 0;
            // Effects before interaction.
            rewardToken.transfer(user, reward);
            emit RewardClaimed(user, reward);
        }
    }

    // ─── Emergency ────────────────────────────────────────────────────────

    function emergencyWithdraw() external nonReentrant {
        uint256 amount = stakedAmount[msg.sender];
        require(amount > 0, "Staking: nothing staked");
        stakedAmount[msg.sender] = 0;
        totalStaked              -= amount;
        rewards[msg.sender]      = 0;
        stakingToken.transfer(msg.sender, amount);
        emit EmergencyWithdraw(msg.sender, amount);
    }

    // ─── Views ────────────────────────────────────────────────────────────

    function earned(address account) public view returns (uint256) {
        return stakedAmount[account] * (_rewardPerToken() - userRewardPerTokenPaid[account]) / 1e18
               + rewards[account];
    }

    function apr() external view returns (uint256) {
        if (totalStaked == 0) return 0;
        return rewardRate * 365 days * 1e18 / totalStaked;
    }

    // ─── Admin ────────────────────────────────────────────────────────────

    function setRewardRate(uint256 newRate) external update(address(0)) onlyOwner {
        rewardRate = newRate;
        emit RewardRateUpdated(newRate);
    }

    // ─── Ownership (2-step — S-MED-01) ────────────────────────────────────

    /// @notice Begin a 2-step ownership transfer. New owner must call `acceptOwnership()`.
    function transferOwnership(address newOwner) external onlyOwner {
        require(newOwner != address(0), "Staking: zero address");
        pendingOwner = newOwner;
        emit OwnershipTransferStarted(owner, newOwner);
    }

    /// @notice Complete the 2-step ownership transfer. Only callable by `pendingOwner`.
    function acceptOwnership() external {
        require(msg.sender == pendingOwner, "Staking: not pending owner");
        emit OwnershipTransferred(owner, pendingOwner);
        owner        = pendingOwner;
        pendingOwner = address(0);
    }

    function depositRewards(uint256 amount) external onlyOwner {
        rewardToken.transferFrom(msg.sender, address(this), amount);
    }

    // ─── Internal ─────────────────────────────────────────────────────────

    function _rewardPerToken() internal view returns (uint256) {
        if (totalStaked == 0) return rewardPerTokenStored;
        return rewardPerTokenStored + rewardRate * (block.timestamp - lastUpdateTime) * 1e18 / totalStaked;
    }
}