// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../ZRC20Base.sol";

contract ConcreteZRC20 is ZRC20Base {
    constructor(string memory name_, string memory symbol_, uint8 dec_)
        ZRC20Base(name_, symbol_, dec_)
    {}
    function mint(address to, uint256 amt) external { _mint(to, amt); }
    function burn(address from, uint256 amt) external { _burn(from, amt); }
}

contract ZRC20BaseTest is Test {
    ConcreteZRC20 token;

    address alice = address(0xA11CE);
    address bob   = address(0xB0B);

    function setUp() public {
        token = new ConcreteZRC20("Test Token", "TTK", 18);
        token.mint(alice, 1_000_000 ether);
    }

    function test_name_symbol_decimals() public view {
        assertEq(token.name(), "Test Token");
        assertEq(token.symbol(), "TTK");
        assertEq(token.decimals(), 18);
    }

    function test_total_supply_after_mint() public view {
        assertEq(token.totalSupply(), 1_000_000 ether);
    }

    function test_balance_of() public view {
        assertEq(token.balanceOf(alice), 1_000_000 ether);
    }

    function test_transfer() public {
        vm.prank(alice);
        token.transfer(bob, 100 ether);
        assertEq(token.balanceOf(bob), 100 ether);
        assertEq(token.balanceOf(alice), 1_000_000 ether - 100 ether);
    }

    function test_transfer_insufficient_reverts() public {
        vm.prank(bob);
        vm.expectRevert();
        token.transfer(alice, 1);
    }

    function test_approve_and_transferFrom() public {
        vm.prank(alice);
        token.approve(bob, 500 ether);
        assertEq(token.allowance(alice, bob), 500 ether);
        vm.prank(bob);
        token.transferFrom(alice, bob, 300 ether);
        assertEq(token.balanceOf(bob), 300 ether);
        assertEq(token.allowance(alice, bob), 200 ether);
    }

    function test_transferFrom_exceeds_allowance_reverts() public {
        vm.prank(alice);
        token.approve(bob, 100 ether);
        vm.prank(bob);
        vm.expectRevert();
        token.transferFrom(alice, bob, 101 ether);
    }

    function test_burn_reduces_supply() public {
        token.burn(alice, 200 ether);
        assertEq(token.totalSupply(), 1_000_000 ether - 200 ether);
    }

    function test_burn_exceeds_balance_reverts() public {
        vm.expectRevert();
        token.burn(alice, 2_000_000 ether);
    }

    function test_batch_transfer() public {
        address[] memory recipients = new address[](3);
        uint256[] memory amounts    = new uint256[](3);
        recipients[0] = address(0x1);
        recipients[1] = address(0x2);
        recipients[2] = address(0x3);
        amounts[0] = 10 ether;
        amounts[1] = 20 ether;
        amounts[2] = 30 ether;
        vm.prank(alice);
        token.batchTransfer(recipients, amounts);
        assertEq(token.balanceOf(address(0x1)), 10 ether);
        assertEq(token.balanceOf(address(0x2)), 20 ether);
        assertEq(token.balanceOf(address(0x3)), 30 ether);
    }

    function test_batch_transfer_length_mismatch_reverts() public {
        address[] memory r = new address[](2);
        uint256[] memory a = new uint256[](3);
        vm.prank(alice);
        vm.expectRevert();
        token.batchTransfer(r, a);
    }

    function test_transfer_to_zero_reverts() public {
        vm.prank(alice);
        vm.expectRevert();
        token.transfer(address(0), 1 ether);
    }
}
