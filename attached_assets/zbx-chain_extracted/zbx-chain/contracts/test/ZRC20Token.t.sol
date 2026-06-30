// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../ZRC20Token.sol";

contract ZRC20TokenTest is Test {
    ZRC20Token token;

    address owner = address(this);
    address alice = address(0xA11CE);
    address bob   = address(0xB0B);

    function setUp() public {
        token = new ZRC20Token(
            "TestToken",
            "TTK",
            18,
            1_000_000 ether,   // initialSupply (minted to owner)
            10_000_000 ether,  // mintCap
            "ipfs://logo"
        );
    }

    // ── Basic ERC-20 ──────────────────────────────────────────────────────

    function test_initial_supply_minted_to_owner() public view {
        assertEq(token.balanceOf(owner), 1_000_000 ether);
        assertEq(token.totalSupply(), 1_000_000 ether);
    }

    function test_transfer() public {
        token.transfer(alice, 1_000 ether);
        assertEq(token.balanceOf(alice), 1_000 ether);
        assertEq(token.balanceOf(owner), 999_000 ether);
    }

    function test_approve_and_transferFrom() public {
        token.transfer(alice, 500 ether);
        vm.prank(alice);
        token.approve(bob, 200 ether);
        vm.prank(bob);
        token.transferFrom(alice, bob, 150 ether);
        assertEq(token.balanceOf(bob), 150 ether);
        assertEq(token.allowance(alice, bob), 50 ether);
    }

    // ── Mint ─────────────────────────────────────────────────────────────

    function test_owner_can_mint() public {
        token.mint(alice, 500 ether);
        assertEq(token.balanceOf(alice), 500 ether);
    }

    function test_mint_cap_enforced() public {
        vm.expectRevert();
        token.mint(alice, 10_000_000 ether); // exceeds cap (1M already minted)
    }

    function test_non_owner_cannot_mint() public {
        vm.prank(alice);
        vm.expectRevert();
        token.mint(bob, 100 ether);
    }

    // ── Burn ─────────────────────────────────────────────────────────────

    function test_holder_can_burn() public {
        token.transfer(alice, 1_000 ether);
        vm.prank(alice);
        token.burn(400 ether);
        assertEq(token.balanceOf(alice), 600 ether);
        assertEq(token.totalSupply(), 999_600 ether);
    }

    function test_burn_with_allowance() public {
        token.transfer(alice, 500 ether);
        vm.prank(alice);
        token.approve(bob, 300 ether);
        vm.prank(bob);
        token.burnFrom(alice, 200 ether);
        assertEq(token.balanceOf(alice), 300 ether);
        assertEq(token.allowance(alice, bob), 100 ether);
    }

    // ── Freeze ────────────────────────────────────────────────────────────

    function test_freeze_blocks_send() public {
        token.transfer(alice, 500 ether);
        token.freeze(alice);
        assertTrue(token.isFrozen(alice));
        vm.prank(alice);
        vm.expectRevert();
        token.transfer(bob, 100 ether);
    }

    function test_freeze_blocks_receive() public {
        token.freeze(alice);
        vm.expectRevert();
        token.transfer(alice, 100 ether);
    }

    function test_unfreeze_restores_transfers() public {
        token.transfer(alice, 500 ether);
        token.freeze(alice);
        token.unfreeze(alice);
        vm.prank(alice);
        token.transfer(bob, 100 ether);
        assertEq(token.balanceOf(bob), 100 ether);
    }

    // ── Pause ─────────────────────────────────────────────────────────────

    function test_pause_blocks_all_transfers() public {
        token.transfer(alice, 500 ether);
        token.pause();
        vm.prank(alice);
        vm.expectRevert();
        token.transfer(bob, 100 ether);
    }

    function test_unpause_restores() public {
        token.transfer(alice, 500 ether);
        token.pause();
        token.unpause();
        vm.prank(alice);
        token.transfer(bob, 100 ether);
        assertEq(token.balanceOf(bob), 100 ether);
    }

    // ── Time-lock ────────────────────────────────────────────────────────

    function test_locked_tokens_cannot_be_sent() public {
        token.transfer(alice, 500 ether);
        vm.prank(alice);
        token.lock(300 ether, block.timestamp + 1 hours);
        vm.prank(alice);
        vm.expectRevert();
        token.transfer(bob, 400 ether); // 400 > 200 free balance
    }

    function test_lock_expires_after_unlock_time() public {
        token.transfer(alice, 500 ether);
        vm.prank(alice);
        token.lock(300 ether, block.timestamp + 1 hours);
        vm.warp(block.timestamp + 2 hours);
        vm.prank(alice);
        token.transfer(bob, 400 ether); // Now unlocked
        assertEq(token.balanceOf(bob), 400 ether);
    }
}
