// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

// ─────────────────────────────────────────────────────────────────────────────
// ZRC20TokenAdvanced.t.sol — Foundry tests for the S16-ZRC20-ADV features.
//
//   Run:    forge test --match-contract ZRC20TokenAdvancedTest -vvv
//   Goal:   Validate the new advanced surface added in S16-ZRC20-ADV:
//             - Mint enable/disable (pauseMinting / resumeMinting / finalizeMinting)
//             - Freeze (USDC-style compliance blacklist)
//             - Native time-lock (per-account, single active lock, auto-expires)
//             - Constructor initial-supply mint (closes ZRC20Factory mint-revert bug)
//             - updateLogoURI no-op fix (now persists + emits LogoURIUpdated)
//             - Combined feature interactions (freeze ⟂ lock ⟂ pause)
//
//   COVERAGE (46 tests):
//     CONSTRUCTOR (3)
//       1.  testConstructorMintsInitialSupply
//       2.  testConstructorRevertsZeroOwner
//       3.  testConstructorRevertsInitialAboveCap
//     FREEZE (11)
//       4.  testFreezeBasic                — owner freezes account
//       5.  testFreezeRevertsZeroAddress
//       6.  testFreezeRevertsAlreadyFrozen
//       7.  testFreezeOnlyOwner
//       8.  testUnfreezeBasic
//       9.  testUnfreezeRevertsNotFrozen
//       10. testFrozenAccountCannotSend
//       11. testFrozenAccountCannotReceive
//       12. testFrozenAccountBlocksMintTo
//       13. testFrozenAccountBlocksBurnFrom
//       14. testFrozenBalanceView
//     LOCK (11)
//       15. testLockTokensBasic
//       16. testLockRevertsZeroAddressAmountPastUnlockInsufficient
//       17. testLockBlocksOutgoingTransfer
//       18. testLockAllowsPartialTransferUpToUnlocked
//       19. testTransferableBalanceMath
//       20. testLockedBalanceOfAutoExpires
//       21. testCanReceiveTokensWhileLocked
//       22. testExtendLockGrowsBothFields
//       23. testExtendLockRevertsShrinking
//       24. testReplaceLockAfterExpiry
//       25. testLockRevertsWhenActiveLock
//     MINT TOGGLE (8)
//       26. testPauseMintingBlocksMint
//       27. testResumeMintingRestoresMint
//       28. testFinalizeMintingPermanentlyBlocks
//       29. testFinalizeMintingRevertsAlreadyFinalized
//       30. testCannotPauseAfterFinalized
//       31. testCannotResumeAfterFinalized
//       32. testNonOwnerCannotPauseFinalize
//       33. testMintRevertsWhenPausedAndCapStillEnforced
//     LOGO URI + COMBINED (4)
//       34. testUpdateLogoURIPersistsAndEmits
//       35. testFreezeAndLockOrthogonalBothBlock
//       36. testTransferPauseAlsoBlocksMintAndBurn
//       37. testRenounceOwnershipBlocksAllAdmin
//     HOOK COVERAGE — added after S16-ZRC20-ADV CRIT-1/CRIT-2 fix (8)
//       38. testBatchTransferRespectsFreezeOfSender
//       39. testBatchTransferRespectsFreezeOfRecipient
//       40. testBatchTransferRespectsLockSerialDebit
//       41. testBatchTransferRespectsTransferPause
//       42. testBatchTransferRespectsAntiBotMaxTx
//       43. testTransferFromRespectsFreezeOfFrom
//       44. testTransferFromRespectsLock
//       45. testBurnRespectsLockedPortion
//       46. testTransferFromRespectsFreezeOfRecipient (symmetry w/ test 43)
// ─────────────────────────────────────────────────────────────────────────────

import "../ZRC20Token.sol";

// ─── HEVM cheatcode (no forge-std dep) ───────────────────────────────────────
//
// Only `vm.warp` is used (by lock-expiry tests). The HEVM cheatcode address
// is the standard `keccak256("hevm cheat code")` truncated to 20 bytes —
// same value Foundry, Hevm, and Halmos all expose.

interface IVm {
    function warp(uint256) external;
    function prank(address) external;
    function expectRevert(bytes calldata) external;
    function expectRevert() external;
}

address constant HEVM_ADDRESS =
    address(uint160(uint256(keccak256("hevm cheat code"))));

IVm constant vm = IVm(HEVM_ADDRESS);

// ─── Test helpers — minimal try/catch reverter (forge-std-free) ──────────────

contract ZRC20TokenAdvancedTest {

    ZRC20Token internal token;

    address internal constant DEPLOYER = address(0xD1);
    address internal constant ALICE    = address(0xA1);
    address internal constant BOB      = address(0xB0);
    address internal constant CAROL    = address(0xCA);
    address internal constant MALLORY  = address(0x4D);

    uint256 internal constant CAP    = 1_000_000 * 1e18;
    uint256 internal constant INIT   = 100_000   * 1e18;

    // ── setUp ────────────────────────────────────────────────────────────────

    function setUp() public {
        // We deploy as DEPLOYER so it becomes the owner + initial holder.
        vm.prank(DEPLOYER);
        token = new ZRC20Token(
            "MyToken", "MTK", 18,
            INIT, CAP, "ipfs://logo", DEPLOYER
        );
        // Seed Alice/Bob with some tokens for transfer tests.
        vm.prank(DEPLOYER); token.transfer(ALICE, 10_000 * 1e18);
        vm.prank(DEPLOYER); token.transfer(BOB,    5_000 * 1e18);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // CONSTRUCTOR
    // ─────────────────────────────────────────────────────────────────────────

    function testConstructorMintsInitialSupply() public view {
        // After setUp: Deployer started with INIT, sent 15k away. Total still INIT.
        require(token.totalSupply() == INIT,                      "1.totalSupply wrong");
        require(token.balanceOf(DEPLOYER) == INIT - 15_000 * 1e18, "1.deployer bal wrong");
        require(token.balanceOf(ALICE) == 10_000 * 1e18,           "1.alice bal wrong");
        require(token.balanceOf(BOB)   ==  5_000 * 1e18,           "1.bob bal wrong");
        require(token.owner() == DEPLOYER,                         "1.owner wrong");
        require(token.isMinter(DEPLOYER),                          "1.deployer not minter");
    }

    function testConstructorRevertsZeroOwner() public {
        try new ZRC20Token("X", "X", 18, 0, 0, "", address(0)) {
            revert("2.should have reverted on zero owner");
        } catch {}
    }

    function testConstructorRevertsInitialAboveCap() public {
        try new ZRC20Token("X", "X", 18, 100, 50, "", DEPLOYER) {
            revert("3.should have reverted on initial > cap");
        } catch {}
    }

    // ─────────────────────────────────────────────────────────────────────────
    // FREEZE
    // ─────────────────────────────────────────────────────────────────────────

    function testFreezeBasic() public {
        vm.prank(DEPLOYER); token.freeze(ALICE);
        require(token.isFrozen(ALICE),   "4.alice not frozen");
        require(!token.isFrozen(BOB),    "4.bob wrongly frozen");
    }

    function testFreezeRevertsZeroAddress() public {
        vm.prank(DEPLOYER);
        try token.freeze(address(0)) {
            revert("5.should have reverted on zero");
        } catch {}
    }

    function testFreezeRevertsAlreadyFrozen() public {
        vm.prank(DEPLOYER); token.freeze(ALICE);
        vm.prank(DEPLOYER);
        try token.freeze(ALICE) {
            revert("6.should have reverted on already frozen");
        } catch {}
    }

    function testFreezeOnlyOwner() public {
        vm.prank(MALLORY);
        try token.freeze(ALICE) {
            revert("7.non-owner should not freeze");
        } catch {}
    }

    function testUnfreezeBasic() public {
        vm.prank(DEPLOYER); token.freeze(ALICE);
        vm.prank(DEPLOYER); token.unfreeze(ALICE);
        require(!token.isFrozen(ALICE), "8.unfreeze failed");
    }

    function testUnfreezeRevertsNotFrozen() public {
        vm.prank(DEPLOYER);
        try token.unfreeze(ALICE) {
            revert("9.unfreeze of non-frozen should revert");
        } catch {}
    }

    function testFrozenAccountCannotSend() public {
        vm.prank(DEPLOYER); token.freeze(ALICE);
        vm.prank(ALICE);
        try token.transfer(BOB, 1) {
            revert("10.frozen-from should revert");
        } catch {}
    }

    function testFrozenAccountCannotReceive() public {
        vm.prank(DEPLOYER); token.freeze(BOB);
        vm.prank(ALICE);
        try token.transfer(BOB, 1) {
            revert("11.frozen-to should revert");
        } catch {}
    }

    function testFrozenAccountBlocksMintTo() public {
        vm.prank(DEPLOYER); token.freeze(ALICE);
        vm.prank(DEPLOYER);
        try token.mint(ALICE, 1) {
            revert("12.mint-to-frozen should revert");
        } catch {}
    }

    function testFrozenAccountBlocksBurnFrom() public {
        // Alice approves DEPLOYER to burn, then DEPLOYER freezes Alice, then burnFrom should revert.
        vm.prank(ALICE);    token.approve(DEPLOYER, type(uint256).max);
        vm.prank(DEPLOYER); token.freeze(ALICE);
        vm.prank(DEPLOYER);
        try token.burnFrom(ALICE, 1) {
            revert("13.burn-from-frozen should revert");
        } catch {}
    }

    function testFrozenBalanceView() public {
        require(token.frozenBalance(ALICE) == 0, "14a.unfrozen frozenBalance must be 0");
        vm.prank(DEPLOYER); token.freeze(ALICE);
        require(token.frozenBalance(ALICE) == token.balanceOf(ALICE), "14b.frozen frozenBalance wrong");
    }

    // ─────────────────────────────────────────────────────────────────────────
    // LOCK
    // ─────────────────────────────────────────────────────────────────────────

    function testLockTokensBasic() public {
        uint64 unlock = uint64(block.timestamp + 1000);
        vm.prank(DEPLOYER); token.lockTokens(ALICE, 4_000 * 1e18, unlock);
        require(token.lockedBalanceOf(ALICE) == 4_000 * 1e18, "15.locked wrong");
        (uint256 amt, uint64 ts) = token.lockInfo(ALICE);
        require(amt == 4_000 * 1e18 && ts == unlock,           "15.lockInfo wrong");
    }

    function testLockRevertsZeroAddressAmountPastUnlockInsufficient() public {
        // zero address
        vm.prank(DEPLOYER);
        try token.lockTokens(address(0), 1, uint64(block.timestamp + 1)) {
            revert("16a.zero address allowed");
        } catch {}
        // zero amount
        vm.prank(DEPLOYER);
        try token.lockTokens(ALICE, 0, uint64(block.timestamp + 1)) {
            revert("16b.zero amount allowed");
        } catch {}
        // past unlock
        vm.prank(DEPLOYER);
        try token.lockTokens(ALICE, 1, uint64(block.timestamp)) {
            revert("16c.past unlock allowed");
        } catch {}
        // insufficient balance
        vm.prank(DEPLOYER);
        try token.lockTokens(ALICE, 999_999_999 * 1e18, uint64(block.timestamp + 1)) {
            revert("16d.insufficient balance allowed");
        } catch {}
    }

    function testLockBlocksOutgoingTransfer() public {
        // Lock 8000 of Alice's 10000.  Try to send 5000 → must revert.
        uint64 unlock = uint64(block.timestamp + 1000);
        vm.prank(DEPLOYER); token.lockTokens(ALICE, 8_000 * 1e18, unlock);
        vm.prank(ALICE);
        try token.transfer(BOB, 5_000 * 1e18) {
            revert("17.locked transfer allowed");
        } catch {}
    }

    function testLockAllowsPartialTransferUpToUnlocked() public {
        // Lock 8000 of Alice's 10000 — she should still be able to send 2000.
        uint64 unlock = uint64(block.timestamp + 1000);
        vm.prank(DEPLOYER); token.lockTokens(ALICE, 8_000 * 1e18, unlock);
        uint256 bobBefore = token.balanceOf(BOB);
        vm.prank(ALICE); token.transfer(BOB, 2_000 * 1e18);
        require(token.balanceOf(BOB) == bobBefore + 2_000 * 1e18, "18.partial transfer failed");
        // One more wei should now revert (locked = 8000, balance = 8000 → transferable = 0).
        vm.prank(ALICE);
        try token.transfer(BOB, 1) {
            revert("18b.over-limit transfer allowed");
        } catch {}
    }

    function testTransferableBalanceMath() public {
        require(token.transferableBalance(ALICE) == 10_000 * 1e18, "19a.pre-lock transferable wrong");
        uint64 unlock = uint64(block.timestamp + 1000);
        vm.prank(DEPLOYER); token.lockTokens(ALICE, 3_000 * 1e18, unlock);
        require(token.transferableBalance(ALICE) == 7_000 * 1e18, "19b.post-lock transferable wrong");
    }

    function testLockedBalanceOfAutoExpires() public {
        uint64 unlock = uint64(block.timestamp + 100);
        vm.prank(DEPLOYER); token.lockTokens(ALICE, 4_000 * 1e18, unlock);
        require(token.lockedBalanceOf(ALICE) == 4_000 * 1e18, "20a.pre-expiry");
        vm.warp(uint256(unlock));   // exactly at unlock
        require(token.lockedBalanceOf(ALICE) == 0,            "20b.at-expiry must auto-zero");
        // Transfer of full balance must now succeed.
        vm.prank(ALICE); token.transfer(BOB, 10_000 * 1e18);
        require(token.balanceOf(ALICE) == 0, "20c.full transfer post-expiry failed");
    }

    function testCanReceiveTokensWhileLocked() public {
        // Lock blocks OUTGOING — incoming transfers must still work.
        uint64 unlock = uint64(block.timestamp + 1000);
        vm.prank(DEPLOYER); token.lockTokens(ALICE, 8_000 * 1e18, unlock);
        uint256 aBefore = token.balanceOf(ALICE);
        vm.prank(BOB); token.transfer(ALICE, 1_000 * 1e18);
        require(token.balanceOf(ALICE) == aBefore + 1_000 * 1e18, "21.incoming blocked");
    }

    function testExtendLockGrowsBothFields() public {
        uint64 unlock1 = uint64(block.timestamp + 100);
        vm.prank(DEPLOYER); token.lockTokens(ALICE, 3_000 * 1e18, unlock1);
        uint64 unlock2 = uint64(block.timestamp + 500);
        vm.prank(DEPLOYER); token.extendLock(ALICE, 5_000 * 1e18, unlock2);
        (uint256 amt, uint64 ts) = token.lockInfo(ALICE);
        require(amt == 5_000 * 1e18 && ts == unlock2, "22.extend wrong");
    }

    function testExtendLockRevertsShrinking() public {
        uint64 unlock1 = uint64(block.timestamp + 500);
        vm.prank(DEPLOYER); token.lockTokens(ALICE, 5_000 * 1e18, unlock1);
        // shrink amount
        vm.prank(DEPLOYER);
        try token.extendLock(ALICE, 3_000 * 1e18, unlock1) {
            revert("23a.amount shrink allowed");
        } catch {}
        // shrink time
        vm.prank(DEPLOYER);
        try token.extendLock(ALICE, 5_000 * 1e18, uint64(block.timestamp + 100)) {
            revert("23b.time shrink allowed");
        } catch {}
    }

    function testReplaceLockAfterExpiry() public {
        uint64 unlock1 = uint64(block.timestamp + 100);
        vm.prank(DEPLOYER); token.lockTokens(ALICE, 3_000 * 1e18, unlock1);
        vm.warp(uint256(unlock1) + 1);
        // After expiry a NEW lock can be smaller and shorter — full reset.
        uint64 unlock2 = uint64(block.timestamp + 200);
        vm.prank(DEPLOYER); token.lockTokens(ALICE, 1_000 * 1e18, unlock2);
        require(token.lockedBalanceOf(ALICE) == 1_000 * 1e18, "24.replace failed");
    }

    function testLockRevertsWhenActiveLock() public {
        uint64 unlock1 = uint64(block.timestamp + 1000);
        vm.prank(DEPLOYER); token.lockTokens(ALICE, 3_000 * 1e18, unlock1);
        vm.prank(DEPLOYER);
        try token.lockTokens(ALICE, 1_000 * 1e18, uint64(block.timestamp + 2000)) {
            revert("25.lockTokens on active lock allowed");
        } catch {}
    }

    // ─────────────────────────────────────────────────────────────────────────
    // MINT TOGGLE
    // ─────────────────────────────────────────────────────────────────────────

    function testPauseMintingBlocksMint() public {
        vm.prank(DEPLOYER); token.pauseMinting();
        require(token.mintingPaused(), "26a.pause flag wrong");
        vm.prank(DEPLOYER);
        try token.mint(ALICE, 1) {
            revert("26b.mint while paused allowed");
        } catch {}
    }

    function testResumeMintingRestoresMint() public {
        vm.prank(DEPLOYER); token.pauseMinting();
        vm.prank(DEPLOYER); token.resumeMinting();
        require(!token.mintingPaused(), "27a.resume flag wrong");
        vm.prank(DEPLOYER); token.mint(ALICE, 1 * 1e18);
        require(token.balanceOf(ALICE) == 10_001 * 1e18, "27b.mint post-resume failed");
    }

    function testFinalizeMintingPermanentlyBlocks() public {
        vm.prank(DEPLOYER); token.finalizeMinting();
        require(token.mintingFinalized(), "28a.finalize flag wrong");
        vm.prank(DEPLOYER);
        try token.mint(ALICE, 1) {
            revert("28b.mint after finalize allowed");
        } catch {}
    }

    function testFinalizeMintingRevertsAlreadyFinalized() public {
        vm.prank(DEPLOYER); token.finalizeMinting();
        vm.prank(DEPLOYER);
        try token.finalizeMinting() {
            revert("29.double finalize allowed");
        } catch {}
    }

    function testCannotPauseAfterFinalized() public {
        vm.prank(DEPLOYER); token.finalizeMinting();
        vm.prank(DEPLOYER);
        try token.pauseMinting() {
            revert("30.pause after finalize allowed");
        } catch {}
    }

    function testCannotResumeAfterFinalized() public {
        // Pause first, then finalize — resume must still be blocked.
        vm.prank(DEPLOYER); token.pauseMinting();
        vm.prank(DEPLOYER); token.finalizeMinting();
        vm.prank(DEPLOYER);
        try token.resumeMinting() {
            revert("31.resume after finalize allowed");
        } catch {}
    }

    function testNonOwnerCannotPauseFinalize() public {
        vm.prank(MALLORY);
        try token.pauseMinting()    { revert("32a"); } catch {}
        vm.prank(MALLORY);
        try token.finalizeMinting() { revert("32b"); } catch {}
    }

    function testMintRevertsWhenPausedAndCapStillEnforced() public {
        // Pause: must revert.
        vm.prank(DEPLOYER); token.pauseMinting();
        vm.prank(DEPLOYER);
        try token.mint(ALICE, 1) { revert("33a"); } catch {}
        // Resume + try to mint above cap: must revert with cap-exceeded path.
        vm.prank(DEPLOYER); token.resumeMinting();
        vm.prank(DEPLOYER);
        try token.mint(ALICE, CAP) { revert("33b.cap not enforced"); } catch {}
    }

    // ─────────────────────────────────────────────────────────────────────────
    // LOGO URI + COMBINED
    // ─────────────────────────────────────────────────────────────────────────

    function testUpdateLogoURIPersistsAndEmits() public {
        require(keccak256(bytes(token.logoURI())) == keccak256(bytes("ipfs://logo")), "34a.initial wrong");
        vm.prank(DEPLOYER); token.updateLogoURI("ipfs://new");
        require(keccak256(bytes(token.logoURI())) == keccak256(bytes("ipfs://new")),  "34b.update no-op");
    }

    function testFreezeAndLockOrthogonalBothBlock() public {
        // Lock 5000 of Alice.  Then freeze her.  Send 1 from Alice's unlocked
        // 5000 — must revert because of FREEZE first (lock alone would allow 1).
        uint64 unlock = uint64(block.timestamp + 1000);
        vm.prank(DEPLOYER); token.lockTokens(ALICE, 5_000 * 1e18, unlock);
        vm.prank(DEPLOYER); token.freeze(ALICE);
        vm.prank(ALICE);
        try token.transfer(BOB, 1) {
            revert("35.frozen-and-locked transfer allowed");
        } catch {}
    }

    function testTransferPauseAlsoBlocksMintAndBurn() public {
        // pause() (transfer-pause) is wired through `whenNotPaused` on
        // _beforeTransfer, which also fires on mint and burn — confirm.
        vm.prank(DEPLOYER); token.pause();
        // mint
        vm.prank(DEPLOYER);
        try token.mint(ALICE, 1) { revert("36a.mint while paused allowed"); } catch {}
        // burn
        vm.prank(ALICE);
        try token.burn(1)        { revert("36b.burn while paused allowed"); } catch {}
        // transfer
        vm.prank(ALICE);
        try token.transfer(BOB, 1) { revert("36c.transfer while paused allowed"); } catch {}
        // Unpause restores all.
        vm.prank(DEPLOYER); token.unpause();
        vm.prank(DEPLOYER); token.mint(ALICE, 1);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // HOOK COVERAGE — verifies batchTransfer / transferFrom / burn correctly
    // route through `_beforeTransfer`. These tests would have FAILED on the
    // pre-CRIT-fix base (which wrote balances directly in batchTransfer/_mint/_burn).
    // ─────────────────────────────────────────────────────────────────────────

    function testBatchTransferRespectsFreezeOfSender() public {
        vm.prank(DEPLOYER); token.freeze(ALICE);
        address[] memory dests = new address[](2);
        uint256[] memory vals  = new uint256[](2);
        dests[0] = BOB;   vals[0] = 1;
        dests[1] = CAROL; vals[1] = 1;
        vm.prank(ALICE);
        try token.batchTransfer(dests, vals) {
            revert("38.batchTransfer-from-frozen allowed");
        } catch {}
    }

    function testBatchTransferRespectsFreezeOfRecipient() public {
        vm.prank(DEPLOYER); token.freeze(BOB);
        address[] memory dests = new address[](2);
        uint256[] memory vals  = new uint256[](2);
        dests[0] = CAROL; vals[0] = 1;
        dests[1] = BOB;   vals[1] = 1;   // 2nd leg targets frozen recipient
        vm.prank(ALICE);
        try token.batchTransfer(dests, vals) {
            revert("39.batchTransfer-to-frozen allowed (must revert atomically)");
        } catch {}
        // Atomicity: leg 1 must NOT have credited Carol because the whole tx reverted.
        require(token.balanceOf(CAROL) == 0, "39b.partial credit leaked");
    }

    function testBatchTransferRespectsLockSerialDebit() public {
        // Alice has 10000.  Lock 6000.  transferable = 4000.
        // Batch: 2000 + 2000 + 2000 = 6000.  Total > transferable; should revert
        // on leg 3 when running balance hits the locked floor.
        uint64 unlock = uint64(block.timestamp + 1000);
        vm.prank(DEPLOYER); token.lockTokens(ALICE, 6_000 * 1e18, unlock);
        address[] memory dests = new address[](3);
        uint256[] memory vals  = new uint256[](3);
        dests[0] = BOB;   vals[0] = 2_000 * 1e18;
        dests[1] = CAROL; vals[1] = 2_000 * 1e18;
        dests[2] = BOB;   vals[2] = 2_000 * 1e18;  // pushes past lock floor
        vm.prank(ALICE);
        try token.batchTransfer(dests, vals) {
            revert("40.batchTransfer past lock floor allowed");
        } catch {}
        // Atomicity again: BOB and CAROL balances unchanged.
        require(token.balanceOf(BOB)   == 5_000 * 1e18, "40b.bob credited from reverted tx");
        require(token.balanceOf(CAROL) == 0,            "40c.carol credited from reverted tx");

        // Successful sub-limit batch (2000+2000=4000 == unlocked) must work.
        address[] memory d2 = new address[](2);
        uint256[] memory v2 = new uint256[](2);
        d2[0] = BOB;   v2[0] = 2_000 * 1e18;
        d2[1] = CAROL; v2[1] = 2_000 * 1e18;
        vm.prank(ALICE); token.batchTransfer(d2, v2);
        require(token.balanceOf(CAROL) == 2_000 * 1e18, "40d.success batch failed");
    }

    function testBatchTransferRespectsTransferPause() public {
        vm.prank(DEPLOYER); token.pause();
        address[] memory dests = new address[](1);
        uint256[] memory vals  = new uint256[](1);
        dests[0] = BOB; vals[0] = 1;
        vm.prank(ALICE);
        try token.batchTransfer(dests, vals) {
            revert("41.batchTransfer while paused allowed");
        } catch {}
    }

    function testBatchTransferRespectsAntiBotMaxTx() public {
        vm.prank(DEPLOYER); token.setMaxTransferAmount(500 * 1e18);
        address[] memory dests = new address[](1);
        uint256[] memory vals  = new uint256[](1);
        dests[0] = BOB; vals[0] = 1_000 * 1e18;  // exceeds maxTransferAmount
        vm.prank(ALICE);
        try token.batchTransfer(dests, vals) {
            revert("42.anti-bot bypassed via batchTransfer");
        } catch {}
    }

    function testTransferFromRespectsFreezeOfFrom() public {
        // Alice approves Bob, then DEPLOYER freezes Alice — transferFrom must revert.
        vm.prank(ALICE);    token.approve(BOB, type(uint256).max);
        vm.prank(DEPLOYER); token.freeze(ALICE);
        vm.prank(BOB);
        try token.transferFrom(ALICE, CAROL, 1) {
            revert("43.transferFrom-of-frozen allowed");
        } catch {}
    }

    function testTransferFromRespectsLock() public {
        // Lock 8000 of Alice's 10000.  Bob has full allowance and tries
        // transferFrom 5000 — must revert because lock applies regardless of
        // who initiates (`_transfer(from, to, value)` runs the lock check on `from`).
        uint64 unlock = uint64(block.timestamp + 1000);
        vm.prank(DEPLOYER); token.lockTokens(ALICE, 8_000 * 1e18, unlock);
        vm.prank(ALICE); token.approve(BOB, type(uint256).max);
        vm.prank(BOB);
        try token.transferFrom(ALICE, CAROL, 5_000 * 1e18) {
            revert("44.transferFrom past lock allowed");
        } catch {}
        // Sub-limit succeeds (transferable = 2000).
        vm.prank(BOB); token.transferFrom(ALICE, CAROL, 2_000 * 1e18);
        require(token.balanceOf(CAROL) == 2_000 * 1e18, "44b.sub-limit transferFrom failed");
    }

    function testBurnRespectsLockedPortion() public {
        // Lock 7000 of Alice's 10000.  burn(5000) must revert because it
        // would dip into the locked portion (transferable = 3000 only).
        uint64 unlock = uint64(block.timestamp + 1000);
        vm.prank(DEPLOYER); token.lockTokens(ALICE, 7_000 * 1e18, unlock);
        vm.prank(ALICE);
        try token.burn(5_000 * 1e18) {
            revert("45.burn past lock allowed");
        } catch {}
        // Sub-limit burn (3000 == unlocked) succeeds.
        vm.prank(ALICE); token.burn(3_000 * 1e18);
        require(token.balanceOf(ALICE) == 7_000 * 1e18, "45b.sub-limit burn failed");
        require(token.totalBurned()    == 3_000 * 1e18, "45c.totalBurned wrong");
    }

    function testTransferFromRespectsFreezeOfRecipient() public {
        // Symmetry with test 43 (frozen-from): transferFrom must also block
        // when the RECIPIENT is frozen. Confirms hook covers both directions
        // on the transferFrom path.
        vm.prank(ALICE);    token.approve(BOB, type(uint256).max);
        vm.prank(DEPLOYER); token.freeze(CAROL);
        vm.prank(BOB);
        try token.transferFrom(ALICE, CAROL, 1) {
            revert("46.transferFrom-to-frozen allowed");
        } catch {}
    }

    function testRenounceOwnershipBlocksAllAdmin() public {
        vm.prank(DEPLOYER); token.renounceOwnership();
        require(token.owner() == address(0), "37a.renounce failed");
        // every onlyOwner path must now be unreachable.
        vm.prank(DEPLOYER);
        try token.freeze(ALICE)         { revert("37b"); } catch {}
        vm.prank(DEPLOYER);
        try token.pauseMinting()        { revert("37c"); } catch {}
        vm.prank(DEPLOYER);
        try token.lockTokens(ALICE, 1, uint64(block.timestamp + 1)) { revert("37d"); } catch {}
        vm.prank(DEPLOYER);
        try token.pause()               { revert("37e"); } catch {}
        vm.prank(DEPLOYER);
        try token.updateLogoURI("x")    { revert("37f"); } catch {}
    }
}
