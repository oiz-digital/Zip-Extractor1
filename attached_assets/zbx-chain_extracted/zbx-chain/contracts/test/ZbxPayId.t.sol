// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../ZbxPayId.sol";

contract ZbxPayIdTest is Test {
    ZbxPayId payId;

    address owner = address(this);
    address alice = address(0xA11CE);
    address bob   = address(0xB0B);

    uint256 constant FEE = 0.01 ether;

    function setUp() public {
        payId = new ZbxPayId();
        vm.deal(alice, 10 ether);
        vm.deal(bob, 10 ether);
    }

    // ── Register ──────────────────────────────────────────────────────────

    function test_register_basic() public {
        vm.prank(alice);
        payId.register{value: FEE}("alice", alice);
        assertEq(payId.resolve("alice"), alice);
    }

    function test_register_sets_owner() public {
        vm.prank(alice);
        payId.register{value: FEE}("alice", alice);
        assertEq(payId.ownerOf("alice"), alice);
    }

    function test_register_duplicate_reverts() public {
        vm.prank(alice);
        payId.register{value: FEE}("alice", alice);
        vm.prank(bob);
        vm.expectRevert();
        payId.register{value: FEE}("alice", bob);
    }

    function test_register_insufficient_fee_reverts() public {
        vm.prank(alice);
        vm.expectRevert();
        payId.register{value: 0.001 ether}("alice", alice);
    }

    function test_register_too_short_name_reverts() public {
        vm.prank(alice);
        vm.expectRevert();
        payId.register{value: FEE}("ab", alice);
    }

    function test_register_invalid_chars_reverts() public {
        vm.prank(alice);
        vm.expectRevert();
        payId.register{value: FEE}("ali@ce", alice);
    }

    // ── Resolve ───────────────────────────────────────────────────────────

    function test_resolve_registered_name() public {
        vm.prank(alice);
        payId.register{value: FEE}("myname", alice);
        assertEq(payId.resolve("myname"), alice);
    }

    function test_resolve_unregistered_returns_zero() public view {
        assertEq(payId.resolve("nobody"), address(0));
    }

    // ── Update wallet ─────────────────────────────────────────────────────

    function test_update_wallet() public {
        vm.prank(alice);
        payId.register{value: FEE}("alice", alice);
        vm.prank(alice);
        payId.updateWallet("alice", bob);
        assertEq(payId.resolve("alice"), bob);
    }

    function test_non_owner_cannot_update_wallet() public {
        vm.prank(alice);
        payId.register{value: FEE}("alice", alice);
        vm.prank(bob);
        vm.expectRevert();
        payId.updateWallet("alice", bob);
    }

    // ── Transfer ──────────────────────────────────────────────────────────

    function test_transfer_pay_id() public {
        vm.prank(alice);
        payId.register{value: FEE}("alice", alice);
        vm.prank(alice);
        payId.transfer("alice", bob);
        assertEq(payId.ownerOf("alice"), bob);
    }

    function test_non_owner_cannot_transfer() public {
        vm.prank(alice);
        payId.register{value: FEE}("alice", alice);
        vm.prank(bob);
        vm.expectRevert();
        payId.transfer("alice", bob);
    }

    // ── Reverse lookup ────────────────────────────────────────────────────

    function test_reverse_lookup() public {
        vm.prank(alice);
        payId.register{value: FEE}("alice", alice);
        vm.prank(alice);
        payId.setPrimaryName("alice");
        assertEq(payId.primaryName(alice), "alice");
    }

    // ── Sub-IDs ───────────────────────────────────────────────────────────

    function test_issue_sub_id() public {
        vm.prank(alice);
        payId.register{value: FEE}("alice", alice);
        vm.prank(alice);
        payId.issueSubId("alice", "shop", bob);
        assertEq(payId.resolve("shop.alice"), bob);
    }

    function test_non_parent_owner_cannot_issue_sub_id() public {
        vm.prank(alice);
        payId.register{value: FEE}("alice", alice);
        vm.prank(bob);
        vm.expectRevert();
        payId.issueSubId("alice", "shop", bob);
    }

    // ── Fee withdrawal ────────────────────────────────────────────────────

    function test_owner_can_withdraw_fees() public {
        vm.prank(alice);
        payId.register{value: FEE}("alice", alice);
        uint256 before = owner.balance;
        payId.withdrawFees(owner);
        assertGt(owner.balance, before);
    }

    function test_non_owner_cannot_withdraw_fees() public {
        vm.prank(alice);
        vm.expectRevert();
        payId.withdrawFees(alice);
    }
}
