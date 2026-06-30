// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { IZRC20 } from "./interfaces/IZRC20.sol";

/// @title ZRC20TokenLocker — Time-lock any ZRC-20 token.
/// @notice Deposit tokens and lock them until a specified unlock timestamp.
///         Useful for team token locks, project reserves, or personal commitment.

contract ZRC20TokenLocker {

    // ─── Events ───────────────────────────────────────────────────────────

    event Locked(uint256 indexed lockId, address indexed owner, address token, uint256 amount, uint64 unlockAt);
    event Unlocked(uint256 indexed lockId, address indexed owner, uint256 amount);
    event LockExtended(uint256 indexed lockId, uint64 newUnlockAt);

    // ─── State ────────────────────────────────────────────────────────────

    struct Lock {
        address owner;
        IZRC20  token;
        uint256 amount;
        uint64  unlockAt;
        bool    withdrawn;
    }

    uint256 public lockCount;
    mapping(uint256 => Lock) public locks;
    mapping(address => uint256[]) public locksByOwner;

    // ─── Lock ─────────────────────────────────────────────────────────────

    function lock(
        address token,
        uint256 amount,
        uint64  unlockAt
    ) external returns (uint256 lockId) {
        require(amount > 0,                      "Locker: zero amount");
        require(unlockAt > block.timestamp,      "Locker: unlock in past");

        lockId = lockCount++;
        locks[lockId] = Lock({
            owner:     msg.sender,
            token:     IZRC20(token),
            amount:    amount,
            unlockAt:  unlockAt,
            withdrawn: false
        });

        locksByOwner[msg.sender].push(lockId);
        IZRC20(token).transferFrom(msg.sender, address(this), amount);
        emit Locked(lockId, msg.sender, token, amount, unlockAt);
    }

    // ─── Withdraw ─────────────────────────────────────────────────────────

    function withdraw(uint256 lockId) external {
        Lock storage l = locks[lockId];
        require(l.owner == msg.sender,          "Locker: not owner");
        require(!l.withdrawn,                   "Locker: already withdrawn");
        require(block.timestamp >= l.unlockAt,  "Locker: still locked");

        l.withdrawn = true;
        l.token.transfer(msg.sender, l.amount);
        emit Unlocked(lockId, msg.sender, l.amount);
    }

    // ─── Extend ───────────────────────────────────────────────────────────

    /// @notice Extend the unlock time (can only push further, not bring forward).
    function extend(uint256 lockId, uint64 newUnlockAt) external {
        Lock storage l = locks[lockId];
        require(l.owner == msg.sender,    "Locker: not owner");
        require(!l.withdrawn,             "Locker: withdrawn");
        require(newUnlockAt > l.unlockAt, "Locker: must extend");
        l.unlockAt = newUnlockAt;
        emit LockExtended(lockId, newUnlockAt);
    }

    function locksOf(address owner) external view returns (uint256[] memory) {
        return locksByOwner[owner];
    }

    function isLocked(uint256 lockId) external view returns (bool) {
        Lock storage l = locks[lockId];
        return !l.withdrawn && block.timestamp < l.unlockAt;
    }
}