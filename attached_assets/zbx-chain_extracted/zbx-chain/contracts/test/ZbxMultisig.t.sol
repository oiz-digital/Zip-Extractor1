// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../ZbxMultisig.sol";

contract ZbxMultisigTest is Test {
    ZbxMultisig multisig;

    address owner1 = address(0x1);
    address owner2 = address(0x2);
    address owner3 = address(0x3);
    address owner4 = address(0x4);
    address owner5 = address(0x5);
    address notOwner = address(0x999);

    address[] owners3;

    function setUp() public {
        owners3 = [owner1, owner2, owner3];
        multisig = new ZbxMultisig(owners3, 2);
        // Fund the multisig
        vm.deal(address(multisig), 10 ether);
    }

    function test_constructor_sets_owners() public view {
        assertTrue(multisig.isOwner(owner1));
        assertTrue(multisig.isOwner(owner2));
        assertTrue(multisig.isOwner(owner3));
        assertFalse(multisig.isOwner(notOwner));
    }

    function test_constructor_sets_required() public view {
        assertEq(multisig.required(), 2);
    }

    function test_zero_required_reverts() public {
        vm.expectRevert();
        new ZbxMultisig(owners3, 0);
    }

    function test_required_exceeds_owners_reverts() public {
        vm.expectRevert();
        new ZbxMultisig(owners3, 4);
    }

    function test_empty_owners_reverts() public {
        address[] memory empty = new address[](0);
        vm.expectRevert();
        new ZbxMultisig(empty, 1);
    }

    function test_submit_transaction() public {
        vm.prank(owner1);
        multisig.submitTransaction(notOwner, 1 ether, "");
        assertEq(multisig.transactions(0).to, notOwner);
        assertEq(multisig.transactions(0).value, 1 ether);
        assertFalse(multisig.transactions(0).executed);
    }

    function test_non_owner_cannot_submit() public {
        vm.prank(notOwner);
        vm.expectRevert();
        multisig.submitTransaction(notOwner, 0, "");
    }

    function test_confirm_transaction() public {
        vm.prank(owner1);
        multisig.submitTransaction(notOwner, 0, "");

        vm.prank(owner2);
        multisig.confirmTransaction(0);

        assertTrue(multisig.confirmed(0, owner2));
    }

    function test_double_confirm_reverts() public {
        vm.prank(owner1);
        multisig.submitTransaction(notOwner, 0, "");
        vm.prank(owner2);
        multisig.confirmTransaction(0);
        vm.prank(owner2);
        vm.expectRevert();
        multisig.confirmTransaction(0);
    }

    function test_execute_after_quorum() public {
        address recipient = address(0xDEAD);
        vm.prank(owner1);
        multisig.submitTransaction(recipient, 1 ether, "");

        vm.prank(owner1);
        multisig.confirmTransaction(0);
        vm.prank(owner2);
        multisig.confirmTransaction(0);

        uint256 before = recipient.balance;
        vm.prank(owner1);
        multisig.executeTransaction(0);

        assertEq(recipient.balance, before + 1 ether);
        assertTrue(multisig.transactions(0).executed);
    }

    function test_execute_without_quorum_reverts() public {
        vm.prank(owner1);
        multisig.submitTransaction(notOwner, 1 ether, "");
        vm.prank(owner1);
        multisig.confirmTransaction(0);

        vm.prank(owner1);
        vm.expectRevert();
        multisig.executeTransaction(0);
    }

    function test_double_execute_reverts() public {
        vm.prank(owner1);
        multisig.submitTransaction(address(0xDEAD), 0, "");
        vm.prank(owner1);
        multisig.confirmTransaction(0);
        vm.prank(owner2);
        multisig.confirmTransaction(0);
        vm.prank(owner1);
        multisig.executeTransaction(0);

        vm.prank(owner1);
        vm.expectRevert();
        multisig.executeTransaction(0);
    }

    function test_revoke_confirmation() public {
        vm.prank(owner1);
        multisig.submitTransaction(notOwner, 0, "");
        vm.prank(owner2);
        multisig.confirmTransaction(0);
        vm.prank(owner2);
        multisig.revokeConfirmation(0);
        assertFalse(multisig.confirmed(0, owner2));
    }

    function test_deposit_emits_event() public {
        vm.expectEmit(true, false, false, true);
        emit ZbxMultisig.Deposit(address(this), 1 ether);
        (bool ok,) = address(multisig).call{value: 1 ether}("");
        assertTrue(ok);
    }
}
