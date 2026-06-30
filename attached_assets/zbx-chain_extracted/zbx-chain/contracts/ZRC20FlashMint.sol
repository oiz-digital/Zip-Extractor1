// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { ZRC20Base }              from "./ZRC20Base.sol";
import { IZRC20 }                 from "./interfaces/IZRC20.sol";
import { IERC3156FlashLender }    from "./interfaces/IERC3156FlashLender.sol";
import { IERC3156FlashBorrower }  from "./interfaces/IERC3156FlashBorrower.sol";

/// @title ZRC20FlashMint — abstract ERC-3156 flash-mint mixin for ZRC-20 tokens.
/// @notice Adds `maxFlashLoan` / `flashFee` / `flashLoan` to any token
///         that extends `ZRC20Base`. Atomically mints the principal to
///         the borrower, invokes the EIP-3156 callback, and burns the
///         principal + fee back from the borrower in the same
///         transaction. The fee is either burned (deflationary) or
///         transferred to a configured fee recipient, depending on
///         operator configuration.
///
/// @dev Pattern (canonical OpenZeppelin ERC20FlashMint, adapted to ZRC-20):
///   ```
///   _mint(receiver, amount);
///   require(receiver.onFlashLoan(...) == CALLBACK_SUCCESS);
///   _spendAllowance(receiver, address(this), amount + fee);
///   if (fee == 0 || flashFeeRecipient == 0) {
///       _burn(receiver, amount + fee);   // deflationary
///   } else {
///       _burn(receiver, amount);
///       _transfer(receiver, flashFeeRecipient, fee);
///   }
///   ```
///   The borrower MUST hold `fee` worth of tokens BEFORE calling
///   `flashLoan` because the principal is exactly `amount` and the
///   repayment is `amount + fee` — the extra `fee` comes from the
///   borrower's preexisting balance (canonical EIP-3156 semantics).
///
/// @dev Owner gating: setters (`_setFlashFeeBps`, `_setFlashMintCap`,
///   `_setFlashFeeRecipient`, `_setFlashMintPaused`) are
///   `internal virtual`. Concrete subclasses MUST expose external
///   wrappers gated by their own access-control mechanism (e.g.
///   `Ownable2Step.onlyOwner`). This keeps the mixin agnostic of the
///   subclass's owner model — `ZRC20Base` itself does NOT inherit any
///   ownership module (its `_owner()` virtual returns `address(0)` by
///   default), so this mixin cannot assume one exists either.
///
/// @dev Reentrancy: nested `flashLoan` calls are blocked via a single
///   `_flashStatus` slot (0 = idle, 1 = in-flight). The OZ canonical
///   implementation deliberately omits this guard (arguing that a
///   nested flash needs both extra cap headroom AND its own approval),
///   but ZRC-20 family tokens may have subclass-overridden hooks
///   (`_beforeTransfer` / `_afterTransfer`) with side effects that
///   make explicit reentrancy protection cheap audit insurance.
///
/// @dev Pause: `_setFlashMintPaused(true)` makes `maxFlashLoan` return
///   0 AND makes `flashLoan` revert immediately with `FlashMintPaused`.
///   This is the operator emergency-stop. `flashFee` continues to
///   return the configured fee for read-side off-chain probing — the
///   pause is enforced at the lend point, not the quote point.
///
/// @dev Deployment policy (S22c architect note — operational hardening,
///   not a code defect): production subclasses SHOULD initialize
///   `_setFlashMintPaused(true)` in their constructor AND configure
///   `_setFlashFeeBps` / `_setFlashMintCap` / `_setFlashFeeRecipient`
///   BEFORE the operator un-pauses. This mixin does NOT auto-pause on
///   deploy because the constructor surface belongs to the subclass
///   (and the EVM-zero defaults — paused=false, cap=0=headroom-only,
///   fee=0, recipient=address(0)) would otherwise expose a freshly
///   deployed subclass to uncapped fee-free flash mints. Concrete
///   subclasses' factory tests SHOULD assert `flashMintPaused() == true`
///   immediately post-construction. Tracked as
///   S22c-FOLLOWUP-PRODUCTION-SUBCLASS in AUDIT_2026-04-30.md.
///
/// @custom:zbx-chain 8989
/// @custom:eip       3156
abstract contract ZRC20FlashMint is ZRC20Base, IERC3156FlashLender {

    // ─── Constants ────────────────────────────────────────────────────────

    /// @dev Magic return value the borrower's `onFlashLoan` MUST return.
    bytes32 internal constant CALLBACK_SUCCESS =
        keccak256("ERC3156FlashBorrower.onFlashLoan");

    /// @dev Hard ceiling on the configurable fee in basis points.
    ///      10_000 bps = 100%, so 1_000 bps = 10% absolute max. The
    ///      recommended production default is 9 bps (0.09%), matching
    ///      the Aave V2 / Maker DAI flash-loan fee conventions.
    uint256 internal constant MAX_FLASH_FEE_BPS = 1_000;

    /// @dev Reentrancy status values for the `_flashStatus` slot.
    uint256 internal constant _FLASH_IDLE       = 0;
    uint256 internal constant _FLASH_INFLIGHT   = 1;

    // ─── Storage ──────────────────────────────────────────────────────────

    /// @dev Fee in basis points (1 bps = 0.01%). Default 0 (no fee)
    ///      until operator calls `_setFlashFeeBps`.
    uint256 internal _flashFeeBps;

    /// @dev Maximum borrowable per single flash. 0 = no extra cap (only
    ///      the totalSupply-headroom limit applies). Owner can set a
    ///      tighter cap.
    uint256 internal _flashMintCap;

    /// @dev Address that receives the fee. address(0) = burn the fee
    ///      (deflationary). Default address(0).
    address internal _flashFeeRecipient;

    /// @dev Operator emergency pause. When true, `maxFlashLoan` returns
    ///      0 AND `flashLoan` reverts with `FlashMintPaused`.
    bool internal _flashMintPaused;

    /// @dev Reentrancy guard slot.
    uint256 private _flashStatus;

    // ─── Events ───────────────────────────────────────────────────────────

    event FlashLoanCompleted(
        address indexed receiver,
        address indexed initiator,
        address indexed token,
        uint256 amount,
        uint256 fee,
        address feeRecipient   // address(0) = fee burned
    );
    event FlashFeeBpsUpdated(uint256 oldBps, uint256 newBps);
    event FlashMintCapUpdated(uint256 oldCap, uint256 newCap);
    event FlashFeeRecipientUpdated(address oldRecipient, address newRecipient);
    event FlashMintPausedUpdated(bool oldPaused, bool newPaused);

    // ─── Errors ───────────────────────────────────────────────────────────

    error FlashFeeBpsTooHigh(uint256 requested, uint256 max);
    error FlashMintExceedsCap(uint256 requested, uint256 maxAllowed);
    error FlashUnsupportedToken(address token);
    error FlashCallbackFailed(bytes32 returned);
    error FlashReentrancy();
    error FlashMintPaused();

    // ─── IERC3156FlashLender views ────────────────────────────────────────

    /// @inheritdoc IERC3156FlashLender
    /// @dev Returns 0 for unsupported tokens (read-side safety per EIP).
    ///      Returns 0 when paused. Otherwise = min(_flashMintCap,
    ///      type(uint256).max - totalSupply()).
    function maxFlashLoan(address token) public view virtual override returns (uint256) {
        if (token != address(this)) return 0;
        if (_flashMintPaused) return 0;
        uint256 headroom = type(uint256).max - totalSupply();
        if (_flashMintCap == 0) return headroom;
        return _flashMintCap < headroom ? _flashMintCap : headroom;
    }

    /// @inheritdoc IERC3156FlashLender
    /// @dev Reverts when token is not this contract (per EIP-3156).
    ///      Continues to return the configured fee even when paused —
    ///      the pause is enforced at lend time, not quote time, so
    ///      off-chain quoting / UI probing remains stable.
    function flashFee(address token, uint256 amount) public view virtual override returns (uint256) {
        if (token != address(this)) revert FlashUnsupportedToken(token);
        return (amount * _flashFeeBps) / 10_000;
    }

    /// @inheritdoc IERC3156FlashLender
    function flashLoan(
        IERC3156FlashBorrower receiver,
        address token,
        uint256 amount,
        bytes calldata data
    ) external virtual override returns (bool) {
        // ─── Reentrancy guard ───
        if (_flashStatus == _FLASH_INFLIGHT) revert FlashReentrancy();
        _flashStatus = _FLASH_INFLIGHT;

        // ─── Pre-flight checks ───
        if (token != address(this)) revert FlashUnsupportedToken(token);
        if (_flashMintPaused) revert FlashMintPaused();

        uint256 cap = maxFlashLoan(token);
        if (amount > cap) revert FlashMintExceedsCap(amount, cap);

        uint256 fee = flashFee(token, amount);
        address receiverAddr = address(receiver);

        // ─── Mint principal to borrower ───
        // _mint runs ZRC20Base._beforeTransfer(address(0), receiver, amount)
        // so any subclass freeze/pause/lock policy applies to the borrower.
        _mint(receiverAddr, amount);

        // ─── Borrower callback ───
        // Forward all gas implicitly (per EIP-3156). Borrower MUST:
        //   1. Do its work using the freshly-minted principal.
        //   2. Call IZRC20(token).approve(msg.sender, amount + fee)
        //      from inside onFlashLoan (msg.sender == this contract).
        //   3. Return CALLBACK_SUCCESS.
        bytes32 ret = receiver.onFlashLoan(msg.sender, token, amount, fee, data);
        if (ret != CALLBACK_SUCCESS) revert FlashCallbackFailed(ret);

        // ─── Pull repayment + burn / transfer fee ───
        // _spendAllowance reverts with "ZRC20: insufficient allowance"
        // if borrower didn't approve enough.
        _spendAllowance(receiverAddr, address(this), amount + fee);

        address recipient = _flashFeeRecipient;
        if (fee == 0 || recipient == address(0)) {
            // Burn principal + fee (deflationary fee model).
            // Borrower must hold `fee` worth of tokens BEFORE the loan
            // because _mint added only `amount`. _burn checks balance.
            _burn(receiverAddr, amount + fee);
        } else {
            // Burn principal, transfer fee to recipient.
            // Borrower must still hold `fee` worth pre-loan (same reason).
            _burn(receiverAddr, amount);
            _transfer(receiverAddr, recipient, fee);
        }

        emit FlashLoanCompleted(receiverAddr, msg.sender, token, amount, fee, recipient);

        // ─── Reset reentrancy guard ───
        _flashStatus = _FLASH_IDLE;
        return true;
    }

    // ─── Public read-side views ───────────────────────────────────────────

    function flashFeeBps()        external view returns (uint256) { return _flashFeeBps; }
    function flashMintCap()       external view returns (uint256) { return _flashMintCap; }
    function flashFeeRecipient()  external view returns (address) { return _flashFeeRecipient; }
    function flashMintPaused()    external view returns (bool)    { return _flashMintPaused; }

    // ─── Internal owner-gated setters ─────────────────────────────────────
    //
    // Subclasses MUST expose external wrappers gated by their own
    // access-control mechanism. Example:
    //
    //   function setFlashFeeBps(uint256 bps) external onlyOwner {
    //       _setFlashFeeBps(bps);
    //   }

    function _setFlashFeeBps(uint256 newBps) internal virtual {
        if (newBps > MAX_FLASH_FEE_BPS) revert FlashFeeBpsTooHigh(newBps, MAX_FLASH_FEE_BPS);
        uint256 old = _flashFeeBps;
        _flashFeeBps = newBps;
        emit FlashFeeBpsUpdated(old, newBps);
    }

    function _setFlashMintCap(uint256 newCap) internal virtual {
        uint256 old = _flashMintCap;
        _flashMintCap = newCap;
        emit FlashMintCapUpdated(old, newCap);
    }

    function _setFlashFeeRecipient(address newRecipient) internal virtual {
        address old = _flashFeeRecipient;
        _flashFeeRecipient = newRecipient;
        emit FlashFeeRecipientUpdated(old, newRecipient);
    }

    function _setFlashMintPaused(bool paused_) internal virtual {
        bool old = _flashMintPaused;
        _flashMintPaused = paused_;
        emit FlashMintPausedUpdated(old, paused_);
    }

    // ─── EIP-165 supportsInterface (S21 chain) ────────────────────────────

    function supportsInterface(bytes4 interfaceId) public pure virtual override returns (bool) {
        return interfaceId == type(IERC3156FlashLender).interfaceId
            || super.supportsInterface(interfaceId);
    }
}
