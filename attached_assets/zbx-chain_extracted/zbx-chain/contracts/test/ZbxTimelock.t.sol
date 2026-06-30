// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../ZbxTimelock.sol";

contract ZbxTimelockTest is Test {
    ZbxTimelock timelock;

    address admin = address(this);
    address alice = address(0xA11CE);

    function setUp() public {
        timelock = new ZbxTimelock(admin, 2 days);
        vm.deal(address(timelock), 10 ether);
    }

    // ── Queue ─────────────────────────────────────────────────────────────

    function test_queue_transaction() public {
        uint256 eta = block.timestamp + 2 days + 1;
        bytes32 txHash = timelock.queueTransaction(
            alice, 0, "transfer(address,uint256)",
            abi.encode(alice, 1 ether), eta
        );
        assertTrue(txHash != bytes32(0));
        assertTrue(timelock.queuedTransactions(txHash));
    }

    function test_queue_eta_too_early_reverts() public {
        vm.expectRevert();
        timelock.queueTransaction(alice, 0, "", "", block.timestamp + 1 days);
    }

    function test_non_admin_cannot_queue() public {
        uint256 eta = block.timestamp + 3 days;
        vm.prank(alice);
        vm.expectRevert();
        timelock.queueTransaction(alice, 0, "", "", eta);
    }

    // ── Execute ───────────────────────────────────────────────────────────

    function test_execute_after_delay() public {
        uint256 eta = block.timestamp + 2 days + 1;
        bytes32 txHash = timelock.queueTransaction(
            alice, 0.5 ether, "", "", eta
        );
        vm.warp(eta + 1);
        uint256 before = alice.balance;
        timelock.executeTransaction{value: 0.5 ether}(alice, 0.5 ether, "", "", eta);
        assertEq(alice.balance, before + 0.5 ether);
        assertFalse(timelock.queuedTransactions(txHash));
    }

    function test_execute_before_eta_reverts() public {
        uint256 eta = block.timestamp + 2 days + 1;
        timelock.queueTransaction(alice, 0, "", "", eta);
        vm.expectRevert();
        timelock.executeTransaction(alice, 0, "", "", eta);
    }

    function test_execute_stale_tx_reverts() public {
        uint256 eta = block.timestamp + 2 days + 1;
        timelock.queueTransaction(alice, 0, "", "", eta);
        // Warp past GRACE_PERIOD
        vm.warp(eta + 15 days);
        vm.expectRevert();
        timelock.executeTransaction(alice, 0, "", "", eta);
    }

    function test_execute_unqueued_tx_reverts() public {
        uint256 eta = block.timestamp + 3 days;
        vm.warp(eta + 1);
        vm.expectRevert();
        timelock.executeTransaction(alice, 0, "", "", eta);
    }

    // ── Cancel ────────────────────────────────────────────────────────────

    function test_cancel_transaction() public {
        uint256 eta = block.timestamp + 2 days + 1;
        bytes32 txHash = timelock.queueTransaction(alice, 0, "", "", eta);
        timelock.cancelTransaction(alice, 0, "", "", eta);
        assertFalse(timelock.queuedTransactions(txHash));
    }

    function test_non_admin_cannot_cancel() public {
        uint256 eta = block.timestamp + 2 days + 1;
        timelock.queueTransaction(alice, 0, "", "", eta);
        vm.prank(alice);
        vm.expectRevert();
        timelock.cancelTransaction(alice, 0, "", "", eta);
    }

    // ── Set delay ─────────────────────────────────────────────────────────

    function test_set_delay_via_self() public {
        // setDelay can only be called by address(this) — the timelock itself
        vm.prank(address(timelock));
        timelock.setDelay(3 days);
        assertEq(timelock.delay(), 3 days);
    }

    function test_set_delay_too_short_reverts() public {
        vm.prank(address(timelock));
        vm.expectRevert();
        timelock.setDelay(1 hours); // below minimum
    }

    // ── Pending admin ─────────────────────────────────────────────────────

    function test_pending_admin_flow() public {
        vm.prank(address(timelock));
        timelock.setPendingAdmin(alice);
        assertEq(timelock.pendingAdmin(), alice);
        vm.prank(alice);
        timelock.acceptAdmin();
        assertEq(timelock.admin(), alice);
    }
}
