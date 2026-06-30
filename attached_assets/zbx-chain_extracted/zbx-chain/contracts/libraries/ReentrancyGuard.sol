// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title  ReentrancyGuard — single-slot non-reentrancy primitive for Zebvix Chain.
/// @notice Inherit and decorate state-mutating external functions with `nonReentrant`.
///
/// @dev    Uses status values `_NOT_ENTERED = 1` / `_ENTERED = 2` instead of
///         `0 / 1` so the storage slot is non-zero on first use. This costs
///         ~17,100 gas on the first invocation but only 5,000 gas on every
///         subsequent invocation (instead of 20,000). Compatible with the
///         OpenZeppelin layout, so audits and tooling already understand it.
///
///         Inlined-guard contracts in this codebase
///         (BridgeVault, ZbxAMM, ZbxEntryPoint, ZbxFaucet, ZbxLendingPool,
///          ZbxStaking, ZusdVault) used `_entry = 1 / 2`. Same semantics —
///         this library lifts the pattern out so newer contracts (ZbxRouter,
///         ZRC20Staking, ZusdStabilityPool) can `import` rather than copy-paste.
///
///         A future hardening pass should migrate the inlined guards onto this
///         library so the audit surface is one file instead of seven.
///
/// @custom:zbx-chain  Chain ID 8989
abstract contract ReentrancyGuard {

    uint256 private constant _NOT_ENTERED = 1;
    uint256 private constant _ENTERED     = 2;

    uint256 private _status;

    constructor() {
        _status = _NOT_ENTERED;
    }

    /// @notice Reverts if the function is re-entered while already executing.
    /// @dev    Cross-function reentrancy is also blocked: the same status
    ///         slot is shared across every `nonReentrant` modifier in the
    ///         inheriting contract.
    modifier nonReentrant() {
        require(_status == _NOT_ENTERED, "ReentrancyGuard: reentrant call");
        _status = _ENTERED;
        _;
        _status = _NOT_ENTERED;
    }

    /// @notice Read-only version: reverts if currently inside a nonReentrant
    ///         function. Useful for view functions that should not be invoked
    ///         from inside a state-mutating call (e.g. price oracles read
    ///         mid-swap return manipulable values).
    modifier nonReentrantView() {
        require(_status == _NOT_ENTERED, "ReentrancyGuard: view during call");
        _;
    }
}
