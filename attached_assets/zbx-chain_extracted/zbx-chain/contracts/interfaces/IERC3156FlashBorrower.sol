// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IERC3156FlashBorrower — canonical ERC-3156 flash-loan receiver.
/// @notice Verbatim signature from EIP-3156. A contract that wishes to
///         receive a flash mint from a `ZRC20FlashMint`-extending token
///         MUST implement this interface and return the magic value
///         `keccak256("ERC3156FlashBorrower.onFlashLoan")` from
///         `onFlashLoan` to signal acceptance and successful repayment
///         preparation. ANY other return value (including zero) MUST
///         cause the lender to revert the flash loan and unwind the
///         pending mint.
///
/// @dev Repayment protocol (lender-side, MUST be matched by borrower):
///        1. Lender calls `_mint(borrower, amount)`.
///        2. Lender calls `borrower.onFlashLoan(initiator, token,
///           amount, fee, data)` — borrower MUST do its work AND
///           call `IZRC20(token).approve(msg.sender, amount + fee)`
///           before returning the magic value.
///        3. Lender pulls `amount + fee` via `_spendAllowance` and
///           burns the principal (and either burns the fee or
///           transfers it to the configured fee recipient).
///        4. Lender returns true.
///
/// @custom:eip 3156
interface IERC3156FlashBorrower {
    /// @notice Receive a flash loan.
    /// @param  initiator  The initiator of the loan (the EOA or
    ///                    contract that called `flashLoan` on the lender).
    /// @param  token      The loan currency (always equals the token
    ///                    contract address itself for ZRC20FlashMint).
    /// @param  amount     The amount of tokens lent.
    /// @param  fee        The additional amount of tokens to repay
    ///                    above the principal.
    /// @param  data       Forwarded `data` argument from `flashLoan`.
    /// @return            keccak256("ERC3156FlashBorrower.onFlashLoan")
    function onFlashLoan(
        address initiator,
        address token,
        uint256 amount,
        uint256 fee,
        bytes calldata data
    ) external returns (bytes32);
}
