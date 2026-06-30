// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { IZRC20 } from "./IZRC20.sol";

/// @title IZRC20Lockable — Native time-lock extension for ZRC-20.
/// @notice Token-issuer can lock a portion of an account's balance until
///         a future timestamp. Locked tokens stay in the holder's wallet
///         (no escrow vault) but cannot be transferred until `unlockTime`.
///
/// @dev    Single active lock per account (simpler + gas-efficient than the
///         multi-lock array model used by the separate ZRC20TokenLocker
///         escrow contract). Auto-expires once `block.timestamp >=
///         unlockTime` — no on-chain unlock tx required. Updates are
///         growth-only: both `amount` and `unlockTime` can only increase
///         while a lock is active. After expiry, a new lock can replace it
///         freely.
interface IZRC20Lockable is IZRC20 {

    // ─── Events ───────────────────────────────────────────────────────────

    event TokensLocked(address indexed account, uint256 amount, uint64 unlockTime);
    event LockExtended(address indexed account, uint256 newAmount, uint64 newUnlockTime);

    // ─── Mutators (owner-only in concrete impl) ───────────────────────────

    /// @notice Place a fresh lock on `account`, or replace an expired one.
    function lockTokens(address account, uint256 amount, uint64 unlockTime) external;

    /// @notice Increase amount and/or unlockTime of an existing active lock.
    ///         Both inputs must be ≥ current values.
    function extendLock(address account, uint256 newAmount, uint64 newUnlockTime) external;

    // ─── Views ────────────────────────────────────────────────────────────

    /// @notice Currently-locked balance — auto-zero once `unlockTime` passes.
    function lockedBalanceOf(address account) external view returns (uint256);

    /// @notice `balanceOf(account) - lockedBalanceOf(account)` (saturating at 0).
    function transferableBalance(address account) external view returns (uint256);

    /// @notice Raw lock data (amount, unlockTime). Returns (0, 0) if never locked.
    function lockInfo(address account) external view returns (uint256 amount, uint64 unlockTime);
}
