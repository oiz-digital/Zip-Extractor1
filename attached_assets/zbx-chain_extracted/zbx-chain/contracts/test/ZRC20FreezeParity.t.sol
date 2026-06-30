// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title ZRC20FreezeParity.t — Sanctions/compliance freeze parity tests for
///        the bridge-wrapped ZRC20 contract (S20 / S16-C deferred).
///
/// @notice ZRC20Token (Zebvix L1 ZRC-20) has full IZRC20Freezable behavior
///         (freeze/unfreeze/isFrozen/frozenBalance + Frozen/Unfrozen events
///         + hook-level enforcement). ZRC20.sol (the bridge-wrapped
///         counterpart on BNB / Polygon / etc.) historically did NOT, so a
///         wallet sanctioned on the L1 token could still freely receive,
///         transfer, mint, and burn the wrapped representation — making
///         the bridge a sanctions-laundering vector. S20 closes this gap.
///
/// @dev    Sandbox cannot run forge. These tests are designed for off-sandbox
///         execution on srv1266996:
///         `forge test --match-path contracts/test/ZRC20FreezeParity.t.sol -vvv`
///
/// @custom:zbx-chain  Chain ID 8989

import { ZRC20 }            from "../ZRC20.sol";
import { IZRC20Freezable }  from "../interfaces/IZRC20Freezable.sol";

interface Hevm {
    function prank(address) external;
    function expectRevert(bytes calldata) external;
    function deal(address, uint256) external;
}

contract ZRC20FreezeParityTest {

    Hevm constant HEVM = Hevm(address(uint160(uint256(keccak256("hevm cheat code")))));

    // ─── Fixtures ─────────────────────────────────────────────────────────

    ZRC20   internal token;
    address internal alice = address(0xA11CE);
    address internal bob   = address(0xB0B);
    address internal mallory = address(0x4A110C); // sanctioned

    function setUp() public {
        // mintCap = 0 → unlimited; deployer (this contract) is owner + minter.
        token = new ZRC20(0);
        // Pre-fund alice and mallory so we can test outbound transfers from
        // both before/after freeze.
        token.mint(alice,   1_000 ether);
        token.mint(mallory, 1_000 ether);
    }

    // ─── 1. Owner gates ───────────────────────────────────────────────────

    function test_Freeze_OnlyOwner() public {
        HEVM.prank(alice);
        HEVM.expectRevert(bytes("ZRC20: not owner"));
        token.freeze(mallory);
    }

    function test_Unfreeze_OnlyOwner() public {
        token.freeze(mallory);          // owner = this
        HEVM.prank(alice);
        HEVM.expectRevert(bytes("ZRC20: not owner"));
        token.unfreeze(mallory);
    }

    // ─── 2. State invariants (mirror ZRC20Token messages exactly) ─────────

    function test_Freeze_ZeroAddressReverts() public {
        HEVM.expectRevert(bytes("ZRC20: zero address"));
        token.freeze(address(0));
    }

    function test_Freeze_DoubleFreezeReverts() public {
        token.freeze(mallory);
        HEVM.expectRevert(bytes("ZRC20: already frozen"));
        token.freeze(mallory);
    }

    function test_Unfreeze_NotFrozenReverts() public {
        HEVM.expectRevert(bytes("ZRC20: not frozen"));
        token.unfreeze(mallory);
    }

    // ─── 3. Hook enforcement — outbound (transfer FROM frozen) ────────────

    function test_Freeze_BlocksTransferFromFrozen() public {
        token.freeze(mallory);
        HEVM.prank(mallory);
        HEVM.expectRevert(bytes("ZRC20: from frozen"));
        token.transfer(alice, 10 ether);
    }

    // ─── 4. Hook enforcement — inbound (transfer TO frozen) ───────────────

    function test_Freeze_BlocksTransferToFrozen() public {
        token.freeze(mallory);
        HEVM.prank(alice);
        HEVM.expectRevert(bytes("ZRC20: to frozen"));
        token.transfer(mallory, 10 ether);
    }

    // ─── 5. Hook enforcement on MINT (S16-ZRC20-ADV CRIT-2 path) ──────────
    //
    // ZRC20Base routes _mint through _beforeTransfer with from = address(0).
    // Freezing the recipient MUST block bridge mints — otherwise the BridgeVault
    // could still mint wrapped ZBX to a sanctioned address.

    function test_Freeze_BlocksMintToFrozen() public {
        token.freeze(mallory);
        HEVM.expectRevert(bytes("ZRC20: to frozen"));
        token.mint(mallory, 50 ether);
    }

    // ─── 6. Hook enforcement on BURN (mirrors USDC compliance) ────────────
    //
    // _burn routes through _beforeTransfer with to = address(0). Freezing
    // the source MUST block burns — preventing a sanctioned holder from
    // exiting via burn-and-bridge-back.

    function test_Freeze_BlocksBurnFromFrozen() public {
        token.freeze(mallory);
        HEVM.prank(mallory);
        HEVM.expectRevert(bytes("ZRC20: from frozen"));
        token.burn(10 ether);
    }

    // ─── 7. Unfreeze restores normal operation ────────────────────────────

    function test_Unfreeze_RestoresTransferAndMint() public {
        token.freeze(mallory);
        token.unfreeze(mallory);

        // Outbound now works.
        HEVM.prank(mallory);
        token.transfer(alice, 10 ether);
        require(token.balanceOf(alice) == 1_010 ether, "outbound after unfreeze");

        // Inbound (mint) now works.
        token.mint(mallory, 50 ether);
        require(token.balanceOf(mallory) == 1_040 ether, "mint after unfreeze");
    }

    // ─── 8. View parity with ZRC20Token ───────────────────────────────────

    function test_FrozenBalance_View() public {
        // Not frozen → returns 0 even with positive balance.
        require(token.frozenBalance(alice) == 0, "frozenBalance != 0 when unfrozen");
        require(token.balanceOf(alice)    == 1_000 ether, "balance fixture");

        // Frozen → returns full balance.
        token.freeze(alice);
        require(token.frozenBalance(alice) == 1_000 ether, "frozenBalance after freeze");
        require(token.isFrozen(alice),                     "isFrozen flag");

        // After unfreeze → back to 0.
        token.unfreeze(alice);
        require(token.frozenBalance(alice) == 0, "frozenBalance after unfreeze");
        require(!token.isFrozen(alice),          "isFrozen flag cleared");
    }

    // ─── 9. EIP-165 — interface discovery ────────────────────────────────

    function test_SupportsInterface_Freezable() public view {
        bytes4 freezableId = type(IZRC20Freezable).interfaceId;
        require(token.supportsInterface(freezableId), "must advertise IZRC20Freezable");
        // Sanity: still advertises the existing ones.
        require(token.supportsInterface(0x01ffc9a7), "must advertise EIP-165");
    }

    // ─── 10. Cross-account isolation ──────────────────────────────────────
    //
    // Freezing mallory MUST NOT affect alice ↔ bob movement.

    function test_Freeze_DoesNotAffectOtherAccounts() public {
        token.freeze(mallory);

        HEVM.prank(alice);
        token.transfer(bob, 100 ether);
        require(token.balanceOf(bob)   == 100 ether, "bob received");
        require(token.balanceOf(alice) == 900 ether, "alice debited");

        // mallory's balance unchanged by alice↔bob movement.
        require(token.balanceOf(mallory) == 1_000 ether, "mallory balance untouched");
    }

    // ─── 11. Hook enforcement on transferFrom — both sides ────────────────
    //
    // S20-Polish per architect review #1 ("compliance evidence completeness").
    // transferFrom routes through `_transfer` which calls `_beforeTransfer`,
    // so the freeze hook is exercised the same as for `transfer`. Two
    // separate tests prove from-frozen and to-frozen blocking through the
    // allowance code path specifically.

    function test_Freeze_BlocksTransferFromWhenFromFrozen() public {
        // alice grants bob allowance, then alice gets frozen → bob's
        // transferFrom must fail with "from frozen".
        HEVM.prank(alice);
        token.approve(bob, 50 ether);

        token.freeze(alice);

        HEVM.prank(bob);
        HEVM.expectRevert(bytes("ZRC20: from frozen"));
        token.transferFrom(alice, bob, 25 ether);
    }

    function test_Freeze_BlocksTransferFromWhenToFrozen() public {
        // alice grants bob allowance, then bob gets frozen → bob's
        // transferFrom into himself must fail with "to frozen". Also
        // proves a *third-party* recipient freeze blocks the leg.
        HEVM.prank(alice);
        token.approve(bob, 50 ether);

        token.freeze(mallory);

        HEVM.prank(bob);
        HEVM.expectRevert(bytes("ZRC20: to frozen"));
        token.transferFrom(alice, mallory, 25 ether);
    }

    // ─── 12. Hook enforcement on batchTransfer leg(s) ─────────────────────
    //
    // S20-Polish per architect review #1. batchTransfer calls _transfer
    // per-leg (per the S16-ZRC20-ADV CRIT fix that closed the hook bypass),
    // so a frozen recipient on ANY leg must abort the whole batch. We use
    // a 2-element batch with a frozen recipient on the SECOND leg to prove
    // per-leg evaluation (not just first-leg shortcut).

    function test_Freeze_BlocksBatchTransferOnAnyLeg() public {
        token.freeze(mallory);

        address[] memory tos    = new address[](2);
        uint256[] memory amts   = new uint256[](2);
        tos[0]  = bob;       amts[0] = 10 ether;   // good leg
        tos[1]  = mallory;   amts[1] = 5 ether;    // frozen recipient

        HEVM.prank(alice);
        HEVM.expectRevert(bytes("ZRC20: to frozen"));
        token.batchTransfer(tos, amts);

        // Whole batch reverts atomically — bob did NOT receive the first leg.
        require(token.balanceOf(bob)   == 0,            "bob must not receive partial batch");
        require(token.balanceOf(alice) == 1_000 ether,  "alice must not be debited");
    }
}
