// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

/// @title  Ownable2Step
/// @author Zebvix Technologies Pvt Ltd
/// @notice Two-step ownership transfer pattern (OpenZeppelin-style) for
///         every Zebvix admin-controlled contract.
///
/// @dev    The default `transferOwnership` semantics on the rest of the
///         codebase used to be a single-call atomic re-assignment. That
///         pattern is unsafe — a typo in `newOwner` permanently bricks
///         the contract because no one can call back in to undo it. The
///         two-step pattern requires the new owner to **prove they can
///         sign from `newOwner`** by calling `acceptOwnership()` from
///         that address, which atomically completes the transfer.
///
///         Until `acceptOwnership()` is called:
///         - the OLD owner retains all `onlyOwner` privileges,
///         - the new owner is recorded as `pendingOwner` only,
///         - the OLD owner can REPLACE the pending owner by calling
///           `transferOwnership(other)` again, or CANCEL by calling
///           `transferOwnership(address(0))`.
///
///         `renounceOwnership()` is intentionally explicit: it sets
///         `owner = address(0)` AND clears `pendingOwner`. This makes
///         the renounce action surface in event-history scans and is
///         one-step (renouncing a contract is irreversible by definition
///         so the second-confirmation pattern adds no safety here).
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:standard   S18 — Ownable2Step migration (S16-deferred)
/// @custom:audits     architect-reviewed; pending VPS forge build/test.
abstract contract Ownable2Step {
    // ─── State ─────────────────────────────────────────────────────────────

    /// @notice Current effective owner. Holds all `onlyOwner` privileges.
    address public owner;

    /// @notice Pending owner. MUST call `acceptOwnership()` from this
    ///         exact address to take over. Zero address means no
    ///         pending transfer.
    address public pendingOwner;

    // ─── Events ────────────────────────────────────────────────────────────

    /// @notice Emitted when `transferOwnership` is invoked. The transfer
    ///         is NOT yet complete — wait for `OwnershipTransferred`.
    event OwnershipTransferStarted(
        address indexed previousOwner,
        address indexed newPendingOwner
    );

    /// @notice Emitted when ownership is finally re-assigned, either via
    ///         `acceptOwnership()` or via `renounceOwnership()`
    ///         (in which case `newOwner == address(0)`).
    event OwnershipTransferred(
        address indexed previousOwner,
        address indexed newOwner
    );

    // ─── Errors ────────────────────────────────────────────────────────────

    /// @notice Caller is not the current `owner`.
    error NotOwner();

    /// @notice Caller is not the current `pendingOwner` (raised by
    ///         `acceptOwnership` from a non-pending address).
    error NotPendingOwner();

    // ─── Modifiers ─────────────────────────────────────────────────────────

    modifier onlyOwner() {
        if (msg.sender != owner) revert NotOwner();
        _;
    }

    // ─── Constructor ───────────────────────────────────────────────────────

    /// @param initialOwner The bootstrap owner. Must be non-zero — a
    ///        contract with no owner cannot be configured. Use
    ///        `renounceOwnership()` post-deploy if zero-owner is desired.
    constructor(address initialOwner) {
        require(initialOwner != address(0), "Ownable2Step: zero initialOwner");
        owner = initialOwner;
        emit OwnershipTransferred(address(0), initialOwner);
    }

    // ─── External — owner ──────────────────────────────────────────────────

    /// @notice Stage `newOwner` as the pending owner. The transfer is NOT
    ///         complete until `newOwner` calls `acceptOwnership()`.
    ///
    ///         Pass `address(0)` to CANCEL a previously-staged transfer.
    function transferOwnership(address newOwner) external virtual onlyOwner {
        pendingOwner = newOwner;
        emit OwnershipTransferStarted(owner, newOwner);
    }

    /// @notice Permanently relinquish ownership. After this call no
    ///         `onlyOwner` function can ever be invoked again. Atomic;
    ///         no two-step confirmation (the action is irreversible by
    ///         definition).
    function renounceOwnership() external virtual onlyOwner {
        address prev = owner;
        owner = address(0);
        pendingOwner = address(0);
        emit OwnershipTransferred(prev, address(0));
    }

    // ─── External — pending owner ──────────────────────────────────────────

    /// @notice Complete a pending ownership transfer. Callable only from
    ///         the address staged as `pendingOwner`. On success:
    ///         - `owner = msg.sender`
    ///         - `pendingOwner = address(0)`
    ///         - `OwnershipTransferred(prev, new)` is emitted.
    function acceptOwnership() external virtual {
        if (msg.sender != pendingOwner) revert NotPendingOwner();
        address prev = owner;
        owner = msg.sender;
        pendingOwner = address(0);
        emit OwnershipTransferred(prev, msg.sender);
    }
}
