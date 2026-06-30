// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../ZRC20.sol";

contract ZRC20Test is Test {
    ZRC20 token;
    address owner = address(this);
    address alice = address(0xA11CE);
    address bob   = address(0xB0B);

    function setUp() public {
        token = new ZRC20(150_000_000 * 1e18);
        token.addMinter(owner);
    }

    // ── Basic ERC-20 ──────────────────────────────────────────────────────

    function test_name_and_symbol() public view {
        assertEq(token.name(), "Zebvix");
        assertEq(token.symbol(), "ZBX");
        assertEq(token.decimals(), 18);
    }

    function test_mint_increases_balance() public {
        token.mint(alice, 1_000 ether);
        assertEq(token.balanceOf(alice), 1_000 ether);
        assertEq(token.totalSupply(), 1_000 ether);
    }

    function test_transfer() public {
        token.mint(alice, 500 ether);
        vm.prank(alice);
        token.transfer(bob, 200 ether);
        assertEq(token.balanceOf(alice), 300 ether);
        assertEq(token.balanceOf(bob), 200 ether);
    }

    function test_transfer_insufficient_balance_reverts() public {
        token.mint(alice, 100 ether);
        vm.prank(alice);
        vm.expectRevert();
        token.transfer(bob, 200 ether);
    }

    function test_approve_and_transferFrom() public {
        token.mint(alice, 1_000 ether);
        vm.prank(alice);
        token.approve(bob, 400 ether);
        vm.prank(bob);
        token.transferFrom(alice, bob, 300 ether);
        assertEq(token.balanceOf(bob), 300 ether);
        assertEq(token.allowance(alice, bob), 100 ether);
    }

    // ── Mint cap ─────────────────────────────────────────────────────────

    function test_mint_cap_enforced() public {
        uint256 cap = token.mintCap();
        vm.expectRevert();
        token.mint(alice, cap + 1);
    }

    // ── Burn ─────────────────────────────────────────────────────────────

    function test_burn_decreases_supply() public {
        token.mint(alice, 1_000 ether);
        vm.prank(alice);
        token.burn(400 ether);
        assertEq(token.balanceOf(alice), 600 ether);
        assertEq(token.totalSupply(), 600 ether);
    }

    function test_burn_more_than_balance_reverts() public {
        token.mint(alice, 100 ether);
        vm.prank(alice);
        vm.expectRevert();
        token.burn(200 ether);
    }

    // ── Freeze ────────────────────────────────────────────────────────────

    function test_freeze_blocks_transfer() public {
        token.mint(alice, 500 ether);
        token.freeze(alice);
        vm.prank(alice);
        vm.expectRevert();
        token.transfer(bob, 100 ether);
    }

    function test_unfreeze_restores_transfer() public {
        token.mint(alice, 500 ether);
        token.freeze(alice);
        token.unfreeze(alice);
        vm.prank(alice);
        token.transfer(bob, 100 ether);
        assertEq(token.balanceOf(bob), 100 ether);
    }

    function test_freeze_zero_address_reverts() public {
        vm.expectRevert();
        token.freeze(address(0));
    }

    // ── Non-minter blocked ────────────────────────────────────────────────

    function test_non_minter_cannot_mint() public {
        vm.prank(alice);
        vm.expectRevert();
        token.mint(bob, 100 ether);
    }

    // ── Ownership ─────────────────────────────────────────────────────────

    function test_owner_can_add_minter() public {
        token.addMinter(alice);
        vm.prank(alice);
        token.mint(bob, 10 ether);
        assertEq(token.balanceOf(bob), 10 ether);
    }
}
