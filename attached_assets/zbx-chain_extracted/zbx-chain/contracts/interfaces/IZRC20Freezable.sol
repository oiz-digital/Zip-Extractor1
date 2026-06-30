// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { IZRC20 } from "./IZRC20.sol";

/// @title IZRC20Freezable — Compliance / sanctions extension for ZRC-20.
/// @notice Token-issuer can freeze a specific account, blocking ALL inbound
///         and outbound transfers (incl. mint to / burn from that account).
///         Modeled on USDC's blacklist semantics.
///
/// @dev    Freeze is per-account, owner-controlled. The zero address can
///         never be frozen (mint/burn use it as sentinel). Frozen state is
///         orthogonal to the lock state (a token can be both frozen and
///         time-locked; either alone is sufficient to block movement).
interface IZRC20Freezable is IZRC20 {

    // ─── Events ───────────────────────────────────────────────────────────

    event Frozen(address indexed account, address indexed by);
    event Unfrozen(address indexed account, address indexed by);

    // ─── Mutators (owner-only in concrete impl) ───────────────────────────

    function freeze(address account) external;
    function unfreeze(address account) external;

    // ─── Views ────────────────────────────────────────────────────────────

    function isFrozen(address account) external view returns (bool);

    /// @notice Returns the account's full balance if frozen, else 0.
    ///         Useful for off-chain compliance dashboards.
    function frozenBalance(address account) external view returns (uint256);
}
