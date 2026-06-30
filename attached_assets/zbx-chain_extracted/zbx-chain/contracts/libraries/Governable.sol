// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title Governable — minimal admin / governance mixin for ZBX contracts.
///
/// @notice Provides three layered roles:
///
///   1. **owner**     — bootstrap admin. EOA or multisig deployed alongside
///                      the contract. Can manage non-critical settings and
///                      can hand control to the timelock.
///   2. **pendingOwner** — 2-step ownership transfer destination. Mitigates
///                      the classic "set owner to wrong address → bricked
///                      contract" footgun.
///   3. **governor**  — the protocol timelock (or any future on-chain governance
///                      executor). Once `setGovernor(timelockAddr)` is called,
///                      every function gated by `onlyAdmin` requires the
///                      timelock as `msg.sender` — the bootstrap owner can no
///                      longer touch it directly.
///
/// `onlyOwner`     — owner only (used for emergency pause and for setting the
///                   governor itself).
/// `onlyGovernor`  — governor only; reverts when the governor is unset.
/// `onlyAdmin`     — owner if `governor == 0`, else governor only. This is
///                   the modifier to use on high-risk admin functions:
///                   pre-launch the deployer can call them, post-launch the
///                   timelock is the only path.
///
/// All transitions emit events for off-chain monitoring.
abstract contract Governable {
    address public owner;
    address public pendingOwner;
    address public governor;

    event OwnershipTransferStarted(address indexed previousOwner, address indexed newPendingOwner);
    event OwnershipTransferred   (address indexed previousOwner, address indexed newOwner);
    event GovernorSet            (address indexed previousGovernor, address indexed newGovernor);

    error NotOwner();
    error NotGovernor();
    error NotAdmin();
    error NotPendingOwner();
    error ZeroAddress();

    constructor(address initialOwner) {
        if (initialOwner == address(0)) revert ZeroAddress();
        owner = initialOwner;
        emit OwnershipTransferred(address(0), initialOwner);
    }

    // ─── Modifiers ────────────────────────────────────────────────────────

    modifier onlyOwner() {
        if (msg.sender != owner) revert NotOwner();
        _;
    }

    modifier onlyGovernor() {
        if (governor == address(0) || msg.sender != governor) revert NotGovernor();
        _;
    }

    /// @notice High-risk admin gate. Routes to the governor once it's set
    ///         (post-launch), otherwise to the bootstrap owner (pre-launch).
    ///         A direct owner call AFTER governance is enabled will revert,
    ///         which is the property production deployments need.
    modifier onlyAdmin() {
        if (governor != address(0)) {
            if (msg.sender != governor) revert NotAdmin();
        } else {
            if (msg.sender != owner) revert NotAdmin();
        }
        _;
    }

    // ─── 2-step ownership transfer ────────────────────────────────────────

    /// @notice Step 1: current owner nominates a successor. Does NOT change
    ///         ownership yet — the successor must explicitly accept. Setting
    ///         `address(0)` cancels any pending transfer.
    function transferOwnership(address newOwner) external onlyOwner {
        pendingOwner = newOwner;
        emit OwnershipTransferStarted(owner, newOwner);
    }

    /// @notice Step 2: nominated successor accepts and becomes the owner.
    function acceptOwnership() external {
        if (msg.sender != pendingOwner) revert NotPendingOwner();
        address previous = owner;
        owner        = pendingOwner;
        pendingOwner = address(0);
        emit OwnershipTransferred(previous, owner);
    }

    // ─── Governor wiring ──────────────────────────────────────────────────

    /// @notice Hand the high-risk admin role to a timelock / governance
    ///         contract. Gated by `onlyAdmin` so the cutover is one-way
    ///         from the owner's perspective:
    ///
    ///         - **Pre-cutover** (`governor == 0`): owner may set the governor
    ///           for the first time. This is the bootstrap path.
    ///         - **Post-cutover** (`governor != 0`): only the current governor
    ///           (the timelock) may change `governor` — typically as part of a
    ///           timelocked proposal that rotates to a new governance contract.
    ///
    ///         This closes the governance-bypass attack the architect flagged
    ///         in session 3: a stolen owner key can no longer simply re-point
    ///         the governor to an attacker-controlled EOA and then immediately
    ///         call any `onlyAdmin` function. After cutover, every governor
    ///         change must itself go through the timelock delay.
    ///
    ///         Setting the governor back to `address(0)` is intentionally
    ///         possible (post-cutover, only via the timelock) so that
    ///         governance can deliberately surrender control back to the owner
    ///         in extreme remediation scenarios. This still requires a
    ///         timelocked proposal to enact.
    function setGovernor(address newGovernor) external onlyAdmin {
        emit GovernorSet(governor, newGovernor);
        governor = newGovernor;
    }
}
