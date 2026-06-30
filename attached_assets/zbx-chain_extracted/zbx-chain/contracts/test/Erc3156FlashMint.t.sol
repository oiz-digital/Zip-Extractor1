// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { ZRC20Base }              from "../ZRC20Base.sol";
import { ZRC20FlashMint }         from "../ZRC20FlashMint.sol";
import { IZRC20 }                 from "../interfaces/IZRC20.sol";
import { IERC3156FlashLender }    from "../interfaces/IERC3156FlashLender.sol";
import { IERC3156FlashBorrower }  from "../interfaces/IERC3156FlashBorrower.sol";

// =============================================================================
// HARNESSES
// =============================================================================

/// @dev Test-only concrete subclass of `ZRC20FlashMint`.
///      Production subclasses MUST gate the four setters via
///      `Ownable2Step.onlyOwner` (or equivalent). For unit tests we
///      expose them publicly so HEVM can configure state directly.
contract FlashMintTokenHarness is ZRC20FlashMint {
    constructor() ZRC20Base("FlashTest", "FT", 18, "") {}

    // TEST-ONLY: arbitrary mint for setup.
    function testMint(address to, uint256 amount) external {
        _mint(to, amount);
    }

    // TEST-ONLY passthroughs (production: gate via Ownable2Step).
    function setFlashFeeBps(uint256 bps)         external { _setFlashFeeBps(bps); }
    function setFlashMintCap(uint256 cap)        external { _setFlashMintCap(cap); }
    function setFlashFeeRecipient(address r)     external { _setFlashFeeRecipient(r); }
    function setFlashMintPaused(bool p)          external { _setFlashMintPaused(p); }
}

/// @dev Reference well-behaved borrower per EIP-3156.
contract WellBehavedBorrower is IERC3156FlashBorrower {
    bytes32 public constant CALLBACK_SUCCESS =
        keccak256("ERC3156FlashBorrower.onFlashLoan");

    address public lastInitiator;
    address public lastToken;
    uint256 public lastAmount;
    uint256 public lastFee;
    bytes   public lastData;

    function onFlashLoan(
        address initiator,
        address token,
        uint256 amount,
        uint256 fee,
        bytes calldata data
    ) external override returns (bytes32) {
        lastInitiator = initiator;
        lastToken     = token;
        lastAmount    = amount;
        lastFee       = fee;
        lastData      = data;
        // Approve lender to pull amount + fee.
        IZRC20(token).approve(msg.sender, amount + fee);
        return CALLBACK_SUCCESS;
    }
}

/// @dev Borrower that returns the WRONG magic value.
contract WrongMagicBorrower is IERC3156FlashBorrower {
    function onFlashLoan(
        address, address token, uint256 amount, uint256 fee, bytes calldata
    ) external override returns (bytes32) {
        IZRC20(token).approve(msg.sender, amount + fee);
        return bytes32(uint256(0xdeadbeef));
    }
}

/// @dev Borrower that NEVER approves the lender to pull repayment.
contract NoApprovalBorrower is IERC3156FlashBorrower {
    bytes32 public constant CALLBACK_SUCCESS =
        keccak256("ERC3156FlashBorrower.onFlashLoan");
    function onFlashLoan(
        address, address, uint256, uint256, bytes calldata
    ) external override returns (bytes32) {
        return CALLBACK_SUCCESS;
    }
}

/// @dev Borrower that attempts a nested `flashLoan` from inside the
///      callback. Should be blocked by the reentrancy guard.
contract ReentrantBorrower is IERC3156FlashBorrower {
    bytes32 public constant CALLBACK_SUCCESS =
        keccak256("ERC3156FlashBorrower.onFlashLoan");
    bool public attacked;
    bool public attackReverted;
    bytes public attackRevertData;

    function onFlashLoan(
        address, address token, uint256 amount, uint256 fee, bytes calldata
    ) external override returns (bytes32) {
        if (!attacked) {
            attacked = true;
            // Nested call: should revert with FlashReentrancy.
            try IERC3156FlashLender(msg.sender)
                .flashLoan(IERC3156FlashBorrower(address(this)), token, 1, "")
            {
                // If it didn't revert, the reentrancy guard is broken.
                attackReverted = false;
            } catch (bytes memory reason) {
                attackReverted = true;
                attackRevertData = reason;
            }
        }
        IZRC20(token).approve(msg.sender, amount + fee);
        return CALLBACK_SUCCESS;
    }
}

/// @dev HEVM cheatcode interface (just `expectRevert` + `prank`).
interface Vm {
    function expectRevert() external;
    function expectRevert(bytes4 selector) external;
    function expectRevert(bytes calldata data) external;
    function prank(address) external;
    function startPrank(address) external;
    function stopPrank() external;
}

// =============================================================================
// TESTS
// =============================================================================

contract Erc3156FlashMintTest {
    Vm constant vm = Vm(0x7109709ECfa91a80626fF3989D68f67F5b1DD12D);

    FlashMintTokenHarness internal token;

    address internal constant ALICE = address(0xA11CE);
    address internal constant BOB   = address(0xB0B);
    address internal constant FEEZ  = address(0xFEE0);

    function setUp() public {
        token = new FlashMintTokenHarness();
    }

    // ─── 1. Happy path: zero fee ──────────────────────────────────────────

    function test_FlashLoan_HappyPath_ZeroFee() public {
        WellBehavedBorrower b = new WellBehavedBorrower();

        uint256 supplyBefore = token.totalSupply();
        bool ok = token.flashLoan(IERC3156FlashBorrower(address(b)), address(token), 1_000 ether, hex"abcd");
        require(ok, "flashLoan returned false");

        // Net: supply unchanged, borrower has 0, no fee paid.
        require(token.totalSupply() == supplyBefore, "supply changed");
        require(token.balanceOf(address(b)) == 0, "borrower balance non-zero");
        require(b.lastAmount() == 1_000 ether, "borrower saw wrong amount");
        require(b.lastFee() == 0, "borrower saw non-zero fee");
        require(b.lastInitiator() == address(this), "wrong initiator");
        require(b.lastToken() == address(token), "wrong token");
    }

    // ─── 2. Happy path: fee burned (no recipient set) ─────────────────────

    function test_FlashLoan_HappyPath_FeeBurned() public {
        WellBehavedBorrower b = new WellBehavedBorrower();
        token.setFlashFeeBps(9);  // 0.09%

        // Pre-fund borrower with the fee amount (canonical EIP-3156).
        uint256 amount = 1_000 ether;
        uint256 fee    = (amount * 9) / 10_000;   // 0.9 ether
        token.testMint(address(b), fee);

        uint256 supplyBefore = token.totalSupply();   // == fee
        token.flashLoan(IERC3156FlashBorrower(address(b)), address(token), amount, "");

        // Net: supply decreased by fee (deflationary), borrower has 0.
        require(token.totalSupply() == supplyBefore - fee, "supply did not decrease by fee");
        require(token.balanceOf(address(b)) == 0, "borrower balance non-zero post-flash");
    }

    // ─── 3. Happy path: fee transferred to recipient ──────────────────────

    function test_FlashLoan_HappyPath_FeeToRecipient() public {
        WellBehavedBorrower b = new WellBehavedBorrower();
        token.setFlashFeeBps(9);
        token.setFlashFeeRecipient(FEEZ);

        uint256 amount = 1_000 ether;
        uint256 fee    = (amount * 9) / 10_000;
        token.testMint(address(b), fee);

        uint256 supplyBefore = token.totalSupply();
        uint256 feezBefore   = token.balanceOf(FEEZ);
        token.flashLoan(IERC3156FlashBorrower(address(b)), address(token), amount, "");

        // Net: supply unchanged, fee landed at recipient, borrower has 0.
        require(token.totalSupply() == supplyBefore, "supply changed (should not when recipient set)");
        require(token.balanceOf(FEEZ) == feezBefore + fee, "recipient did not receive fee");
        require(token.balanceOf(address(b)) == 0, "borrower balance non-zero");
    }

    // ─── 4. Reverts: token unsupported ────────────────────────────────────

    function test_FlashLoan_RevertsWhen_TokenUnsupported() public {
        WellBehavedBorrower b = new WellBehavedBorrower();
        vm.expectRevert(
            abi.encodeWithSelector(ZRC20FlashMint.FlashUnsupportedToken.selector, address(0xBAD))
        );
        token.flashLoan(IERC3156FlashBorrower(address(b)), address(0xBAD), 100, "");
    }

    // ─── 5. Reverts: amount > cap ─────────────────────────────────────────

    function test_FlashLoan_RevertsWhen_ExceedsCap() public {
        WellBehavedBorrower b = new WellBehavedBorrower();
        token.setFlashMintCap(500 ether);

        vm.expectRevert(
            abi.encodeWithSelector(ZRC20FlashMint.FlashMintExceedsCap.selector, 600 ether, 500 ether)
        );
        token.flashLoan(IERC3156FlashBorrower(address(b)), address(token), 600 ether, "");
    }

    // ─── 6. Reverts: callback returns wrong magic ─────────────────────────

    function test_FlashLoan_RevertsWhen_CallbackWrongMagic() public {
        WrongMagicBorrower b = new WrongMagicBorrower();
        vm.expectRevert(
            abi.encodeWithSelector(ZRC20FlashMint.FlashCallbackFailed.selector, bytes32(uint256(0xdeadbeef)))
        );
        token.flashLoan(IERC3156FlashBorrower(address(b)), address(token), 100 ether, "");
    }

    // ─── 7. Reverts: borrower didn't approve repayment ────────────────────

    function test_FlashLoan_RevertsWhen_InsufficientAllowance() public {
        NoApprovalBorrower b = new NoApprovalBorrower();
        // ZRC20Base._spendAllowance reverts with "ZRC20: insufficient allowance".
        vm.expectRevert(bytes("ZRC20: insufficient allowance"));
        token.flashLoan(IERC3156FlashBorrower(address(b)), address(token), 100 ether, "");
    }

    // ─── 8. Reverts: nested flashLoan blocked by reentrancy guard ─────────

    function test_FlashLoan_RevertsWhen_Reentrant() public {
        ReentrantBorrower b = new ReentrantBorrower();
        token.flashLoan(IERC3156FlashBorrower(address(b)), address(token), 100 ether, "");

        // Outer call succeeded (borrower swallowed the inner revert in try/catch).
        require(b.attacked(), "attacker did not attempt reentrancy");
        require(b.attackReverted(), "reentrancy was NOT blocked");
        // The captured revert data MUST be FlashReentrancy() selector.
        bytes memory data = b.attackRevertData();
        require(data.length >= 4, "revert data too short");
        bytes4 sel;
        assembly { sel := mload(add(data, 32)) }
        require(sel == ZRC20FlashMint.FlashReentrancy.selector, "wrong revert selector");
    }

    // ─── 9. maxFlashLoan returns 0 for unsupported token (no revert) ──────

    function test_MaxFlashLoan_UnsupportedToken_ReturnsZero() public view {
        require(token.maxFlashLoan(address(0xDEAD)) == 0, "should return 0 for unsupported");
        require(token.maxFlashLoan(address(0)) == 0,      "should return 0 for zero addr");
    }

    // ─── 10. maxFlashLoan respects headroom + cap ─────────────────────────

    function test_MaxFlashLoan_RespectsHeadroomAndCap() public {
        // No cap set → returns full uint256 headroom (uint256.max - 0).
        require(token.maxFlashLoan(address(token)) == type(uint256).max, "no-cap headroom wrong");

        // Set a cap → returns the cap.
        token.setFlashMintCap(1_000 ether);
        require(token.maxFlashLoan(address(token)) == 1_000 ether, "cap not respected");

        // Mint some supply → headroom shrinks, but cap is still tighter.
        token.testMint(ALICE, 500 ether);
        require(token.maxFlashLoan(address(token)) == 1_000 ether, "cap should still apply");

        // Pause → returns 0.
        token.setFlashMintPaused(true);
        require(token.maxFlashLoan(address(token)) == 0, "paused should return 0");
    }

    // ─── 11. flashFee reverts for unsupported token (per EIP) ─────────────

    function test_FlashFee_RevertsWhen_TokenUnsupported() public {
        vm.expectRevert(
            abi.encodeWithSelector(ZRC20FlashMint.FlashUnsupportedToken.selector, address(0xDEAD))
        );
        token.flashFee(address(0xDEAD), 100);
    }

    // ─── 12. setFlashFeeBps enforces hard ceiling ─────────────────────────

    function test_SetFlashFeeBps_RevertsWhen_TooHigh() public {
        vm.expectRevert(
            abi.encodeWithSelector(ZRC20FlashMint.FlashFeeBpsTooHigh.selector, 1_001, 1_000)
        );
        token.setFlashFeeBps(1_001);

        // Boundary: exactly 1000 bps (10%) is allowed.
        token.setFlashFeeBps(1_000);
        require(token.flashFeeBps() == 1_000, "boundary 1000 bps rejected");
    }

    // ─── 13. Pause blocks flashLoan with explicit error ───────────────────

    function test_FlashLoan_RevertsWhen_Paused() public {
        WellBehavedBorrower b = new WellBehavedBorrower();
        token.setFlashMintPaused(true);
        vm.expectRevert(ZRC20FlashMint.FlashMintPaused.selector);
        token.flashLoan(IERC3156FlashBorrower(address(b)), address(token), 100 ether, "");
    }

    // ─── 14. EIP-165: supportsInterface advertises IERC3156FlashLender ────

    function test_EIP165_AdvertisesFlashLender() public view {
        require(token.supportsInterface(type(IERC3156FlashLender).interfaceId),
                "must advertise IERC3156FlashLender");
        // Inherited from ZRC20Base: IZRC20 + EIP-165 itself.
        require(token.supportsInterface(type(IZRC20).interfaceId), "lost IZRC20 advertisement");
        require(token.supportsInterface(0x01ffc9a7), "lost EIP-165 self-advertisement");
        // Sanity: unknown id returns false.
        require(!token.supportsInterface(0xffffffff), "must reject 0xffffffff");
    }

    // ─── 15. Reverts: recipient mode, borrower NOT pre-funded for fee ─────
    //
    // Architect-requested regression (S22c review polish): explicitly
    // document the fee-prebalance requirement in recipient-fee mode.
    //
    // Sequence:
    //   1. _mint(b, amount)          — b balance: amount
    //   2. callback approves amount+fee
    //   3. _spendAllowance succeeds  — allowance: 0
    //   4. _burn(b, amount)          — b balance: 0
    //   5. _transfer(b, recipient, fee)
    //         ↳ requires b.balanceOf >= fee, BUT b has 0 after step 4
    //         ↳ reverts with "ZRC20: insufficient balance"
    //
    // Compare with test #2 (FeeBurned) which pre-funds b with fee.

    function test_FlashLoan_RevertsWhen_RecipientMode_NoFeePrefund() public {
        WellBehavedBorrower b = new WellBehavedBorrower();
        token.setFlashFeeBps(9);
        token.setFlashFeeRecipient(FEEZ);

        // Borrower has 0 tokens. Recipient mode + non-zero fee.
        // Lender will mint amount, take repayment of amount+fee allowance,
        // burn amount, then try to transfer fee — insufficient balance.
        vm.expectRevert(bytes("ZRC20: insufficient balance"));
        token.flashLoan(IERC3156FlashBorrower(address(b)), address(token), 1_000 ether, "");

        // Sanity: state is fully unwound by the revert (atomicity).
        require(token.totalSupply() == 0, "supply leaked through reverted flash");
        require(token.balanceOf(address(b)) == 0, "borrower balance leaked");
        require(token.balanceOf(FEEZ) == 0, "recipient balance leaked");
    }
}
