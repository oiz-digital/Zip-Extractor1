// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

/// @title  Ownable2Step.t.sol
/// @notice HEVM/forge unit tests for the `Ownable2Step` two-step ownership
///         base contract. Covers bootstrap, transfer-stage, accept,
///         reject-by-non-pending, replace pending mid-flight, cancel via
///         zero-address, renounce, and post-renounce lockdown.
///
/// @dev    NOT executable in the Replit sandbox (no forge binary, no
///         RocksDB capacity). Mandatory off-sandbox: VPS srv1266996
///         `forge test --match-path contracts/test/Ownable2Step.t.sol -vvv`
///         must show all 14 tests passing before merge.
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:standard   S18 — Ownable2Step migration

import { Ownable2Step } from "../Ownable2Step.sol";

interface Hevm {
    function prank(address) external;
    function startPrank(address) external;
    function stopPrank() external;
    function expectRevert(bytes calldata) external;
    function expectRevert(bytes4)         external;
    function expectEmit(bool, bool, bool, bool) external;
    function deal(address, uint256) external;
}

/// @dev Minimal concrete to instantiate the abstract base.
contract OwnableHarness is Ownable2Step {
    uint256 public configValue;

    constructor(address initialOwner) Ownable2Step(initialOwner) {}

    /// @notice Owner-gated mutator used to verify auth post-transfer.
    function setConfig(uint256 v) external onlyOwner {
        configValue = v;
    }
}

contract Ownable2StepTest {

    Hevm  internal constant HEVM = Hevm(address(uint160(uint256(keccak256("hevm cheat code")))));

    address internal constant ALICE = address(0xA11CE);
    address internal constant BOB   = address(0xB0B);
    address internal constant CARL  = address(0xCA21);

    OwnableHarness internal h;

    // Re-declared events (must match base for `expectEmit`).
    event OwnershipTransferStarted(address indexed previousOwner, address indexed newPendingOwner);
    event OwnershipTransferred   (address indexed previousOwner, address indexed newOwner);

    function setUp() public {
        h = new OwnableHarness(ALICE);
    }

    // ─── Bootstrap ─────────────────────────────────────────────────────────

    function test_Bootstrap_OwnerSet() public view {
        require(h.owner()        == ALICE,         "owner != ALICE");
        require(h.pendingOwner() == address(0),    "pendingOwner != 0");
    }

    function test_Bootstrap_RejectsZeroOwner() public {
        // The base reverts with a require-string, not a custom error.
        HEVM.expectRevert(bytes("Ownable2Step: zero initialOwner"));
        new OwnableHarness(address(0));
    }

    // ─── transferOwnership stages, does NOT complete ───────────────────────

    function test_TransferOwnership_StagesOnly() public {
        HEVM.prank(ALICE);
        h.transferOwnership(BOB);

        require(h.owner()        == ALICE, "owner unexpectedly changed");
        require(h.pendingOwner() == BOB,   "pendingOwner != BOB");
    }

    function test_TransferOwnership_OnlyOwnerCanStage() public {
        HEVM.prank(BOB);
        HEVM.expectRevert(Ownable2Step.NotOwner.selector);
        h.transferOwnership(BOB);
    }

    function test_TransferOwnership_EmitsStartedEvent() public {
        HEVM.expectEmit(true, true, false, false);
        emit OwnershipTransferStarted(ALICE, BOB);

        HEVM.prank(ALICE);
        h.transferOwnership(BOB);
    }

    // ─── acceptOwnership ───────────────────────────────────────────────────

    function test_AcceptOwnership_OnlyByPending() public {
        HEVM.prank(ALICE);
        h.transferOwnership(BOB);

        HEVM.prank(CARL);
        HEVM.expectRevert(Ownable2Step.NotPendingOwner.selector);
        h.acceptOwnership();

        // ALICE (the OLD owner) also cannot accept on BOB's behalf.
        HEVM.prank(ALICE);
        HEVM.expectRevert(Ownable2Step.NotPendingOwner.selector);
        h.acceptOwnership();
    }

    function test_AcceptOwnership_CompletesTransfer() public {
        HEVM.prank(ALICE);
        h.transferOwnership(BOB);

        HEVM.prank(BOB);
        h.acceptOwnership();

        require(h.owner()        == BOB,        "owner != BOB");
        require(h.pendingOwner() == address(0), "pendingOwner not cleared");

        // BOB can now invoke onlyOwner functions.
        HEVM.prank(BOB);
        h.setConfig(42);
        require(h.configValue() == 42, "config not set");

        // ALICE has lost privileges.
        HEVM.prank(ALICE);
        HEVM.expectRevert(Ownable2Step.NotOwner.selector);
        h.setConfig(99);
    }

    function test_AcceptOwnership_EmitsTransferredEvent() public {
        HEVM.prank(ALICE);
        h.transferOwnership(BOB);

        HEVM.expectEmit(true, true, false, false);
        emit OwnershipTransferred(ALICE, BOB);

        HEVM.prank(BOB);
        h.acceptOwnership();
    }

    function test_AcceptOwnership_FailsWhenNoPending() public {
        // No transferOwnership call — pendingOwner is the zero-address.
        // Anyone calling acceptOwnership fails with NotPendingOwner.
        HEVM.prank(BOB);
        HEVM.expectRevert(Ownable2Step.NotPendingOwner.selector);
        h.acceptOwnership();
    }

    // ─── Replace pending owner mid-flight ──────────────────────────────────

    function test_TransferOwnership_ReplacesPendingMidFlight() public {
        HEVM.prank(ALICE);
        h.transferOwnership(BOB);

        // ALICE changes her mind and stages CARL instead.
        HEVM.prank(ALICE);
        h.transferOwnership(CARL);

        require(h.pendingOwner() == CARL, "pendingOwner != CARL");

        // BOB can no longer accept.
        HEVM.prank(BOB);
        HEVM.expectRevert(Ownable2Step.NotPendingOwner.selector);
        h.acceptOwnership();

        // CARL accepts and becomes owner.
        HEVM.prank(CARL);
        h.acceptOwnership();
        require(h.owner() == CARL, "owner != CARL");
    }

    // ─── Cancel pending via zero-address ───────────────────────────────────

    function test_TransferOwnership_CancelViaZero() public {
        HEVM.prank(ALICE);
        h.transferOwnership(BOB);

        HEVM.prank(ALICE);
        h.transferOwnership(address(0));

        require(h.pendingOwner() == address(0), "pendingOwner not cancelled");

        HEVM.prank(BOB);
        HEVM.expectRevert(Ownable2Step.NotPendingOwner.selector);
        h.acceptOwnership();
    }

    // ─── renounceOwnership ─────────────────────────────────────────────────

    function test_RenounceOwnership_ClearsBoth() public {
        HEVM.prank(ALICE);
        h.transferOwnership(BOB);  // stage a pending

        HEVM.prank(ALICE);
        h.renounceOwnership();

        require(h.owner()        == address(0), "owner not zero");
        require(h.pendingOwner() == address(0), "pendingOwner not cleared on renounce");
    }

    function test_RenounceOwnership_OnlyByOwner() public {
        HEVM.prank(BOB);
        HEVM.expectRevert(Ownable2Step.NotOwner.selector);
        h.renounceOwnership();
    }

    function test_RenounceOwnership_LocksContract() public {
        HEVM.prank(ALICE);
        h.renounceOwnership();

        // No one can ever invoke onlyOwner again — including ALICE.
        HEVM.prank(ALICE);
        HEVM.expectRevert(Ownable2Step.NotOwner.selector);
        h.setConfig(1);

        // No one can stage a new pending either.
        HEVM.prank(ALICE);
        HEVM.expectRevert(Ownable2Step.NotOwner.selector);
        h.transferOwnership(BOB);
    }
}
