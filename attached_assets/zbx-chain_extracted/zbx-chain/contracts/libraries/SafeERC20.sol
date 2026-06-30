// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title  SafeERC20 — USDT-compatible safe wrapper for ERC-20 transfers.
///
/// @notice The ERC-20 spec says `transfer` / `transferFrom` MUST return a bool,
///         but the world's two largest stablecoins (USDT, BNB) **do not**:
///         their pre-EIP-20 implementations return nothing. Calling them with
///         a `IERC20.transfer(...)` interface will:
///           * succeed silently when the call returns no data (good)
///           * but cause `require(token.transfer(...))` to revert in 0.8+
///             because the ABI decoder fails on empty returndata (bad)
///
///         This library uses low-level `.call(...)` and accepts BOTH:
///           * a true 32-byte boolean return (standard tokens), AND
///           * an empty return (USDT, BNB, OMG, …)
///
///         It reverts on any other shape, on `false` return, or on a failed
///         call. Use `SafeERC20.safeTransfer` / `safeTransferFrom` everywhere
///         the bridge / router / vault touches a third-party token whose
///         conformance is not guaranteed.
///
/// @dev    SEC-2026-05-09 — added in the post-AUDIT_2026-04-30 hardening pass.
///         Slither's `unchecked-transfer` detector is satisfied by every call
///         site that uses these helpers.
///
/// @custom:zbx-chain  Chain ID 8989

interface IERC20Minimal {
    function transfer(address to, uint256 amount) external returns (bool);
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
    function approve(address spender, uint256 amount) external returns (bool);
}

library SafeERC20 {

    /// @dev Reverts if the call fails or the token returns explicit `false`.
    function safeTransfer(IERC20Minimal token, address to, uint256 amount) internal {
        _callOptionalReturn(
            address(token),
            abi.encodeCall(IERC20Minimal.transfer, (to, amount)),
            "SafeERC20: transfer failed"
        );
    }

    function safeTransferFrom(IERC20Minimal token, address from, address to, uint256 amount) internal {
        _callOptionalReturn(
            address(token),
            abi.encodeCall(IERC20Minimal.transferFrom, (from, to, amount)),
            "SafeERC20: transferFrom failed"
        );
    }

    /// @dev USDT requires `approve(spender, 0)` between non-zero approvals.
    ///      `forceApprove` clears first, then sets — safe for both.
    function forceApprove(IERC20Minimal token, address spender, uint256 amount) internal {
        _callOptionalReturn(
            address(token),
            abi.encodeCall(IERC20Minimal.approve, (spender, 0)),
            "SafeERC20: zero approve failed"
        );
        if (amount > 0) {
            _callOptionalReturn(
                address(token),
                abi.encodeCall(IERC20Minimal.approve, (spender, amount)),
                "SafeERC20: approve failed"
            );
        }
    }

    function _callOptionalReturn(address token, bytes memory data, string memory errMsg) private {
        // Token must have code; otherwise an empty-returndata "success" from
        // an EOA would silently be treated as a valid transfer.
        require(token.code.length > 0, "SafeERC20: not a contract");

        (bool ok, bytes memory ret) = token.call(data);
        require(ok, errMsg);

        // Accept either: empty return (USDT-style) OR ABI-encoded true.
        if (ret.length > 0) {
            require(abi.decode(ret, (bool)), errMsg);
        }
    }
}
