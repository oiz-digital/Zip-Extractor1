// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { IERC3156FlashBorrower } from "./IERC3156FlashBorrower.sol";

/// @title IERC3156FlashLender — canonical ERC-3156 flash-loan lender.
/// @notice Verbatim signatures from EIP-3156. The ZRC20FlashMint mixin
///         implements this interface for any ZRC-20 family token that
///         opts into flash-mint capability.
///
/// @dev Per-token semantics (ZRC20FlashMint subset of EIP-3156):
///        * `token` MUST equal the token contract address itself; ANY
///          other value MUST cause `flashFee` and `flashLoan` to revert
///          with `FlashUnsupportedToken(token)` and MUST cause
///          `maxFlashLoan` to return 0 (the EIP allows either; this
///          implementation chose return-0 for read-side safety so that
///          off-chain callers can probe support without paying for a
///          revert).
///        * Fee is computed as floor(amount * flashFeeBps / 10000).
///        * Maximum loan is bounded by both the owner-configured cap
///          AND the totalSupply headroom (uint256.max - totalSupply()).
///        * Reentrancy of `flashLoan` is blocked by an internal status
///          guard; nested flash loans revert with `FlashReentrancy()`.
///
/// @custom:eip 3156
interface IERC3156FlashLender {
    /// @notice The amount of currency available to be lent.
    /// @param  token   The loan currency.
    /// @return         The amount of `token` that can be borrowed
    ///                 (returns 0 for unsupported tokens — does NOT
    ///                 revert, per EIP-3156 §maxFlashLoan).
    function maxFlashLoan(address token) external view returns (uint256);

    /// @notice The fee to be charged for a given loan.
    /// @param  token   The loan currency.
    /// @param  amount  The amount of tokens lent.
    /// @return         The amount of `token` to be charged for the loan,
    ///                 on top of the returned principal.
    /// @dev    MUST revert when `token` is not supported.
    function flashFee(address token, uint256 amount) external view returns (uint256);

    /// @notice Initiate a flash loan.
    /// @param  receiver  The receiver of the tokens in the loan, and
    ///                   the receiver of the callback.
    /// @param  token     The loan currency.
    /// @param  amount    The amount of tokens lent.
    /// @param  data      Arbitrary data structure, intended to contain
    ///                   user-defined parameters.
    /// @return           true on success.
    function flashLoan(
        IERC3156FlashBorrower receiver,
        address token,
        uint256 amount,
        bytes calldata data
    ) external returns (bool);
}
