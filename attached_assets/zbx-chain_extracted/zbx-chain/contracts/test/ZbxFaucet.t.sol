// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../ZbxFaucet.sol";

contract ZbxFaucetTest is Test {
    ZbxFaucet faucet;
    address owner = address(this);
    address alice = address(0xA11CE);
    address bob   = address(0xB0B);

    function setUp() public {
        faucet = new ZbxFaucet{value: 10_000 ether}();
    }

    function test_drip_amount_is_100_zbx() public view {
        assertEq(faucet.DRIP_AMOUNT(), 100 ether);
    }

    function test_cooldown_is_24_hours() public view {
        assertEq(faucet.COOLDOWN(), 24 hours);
    }

    function test_request_dispenses_tokens() public {
        uint256 before = alice.balance;
        vm.prank(alice);
        faucet.request();
        assertEq(alice.balance, before + 100 ether);
    }

    function test_request_increments_counters() public {
        vm.prank(alice);
        faucet.request();
        assertEq(faucet.requestCount(), 1);
        assertEq(faucet.totalDispensed(), 100 ether);
    }

    function test_second_request_within_cooldown_reverts() public {
        vm.prank(alice);
        faucet.request();
        vm.prank(alice);
        vm.expectRevert();
        faucet.request();
    }

    function test_request_after_cooldown_succeeds() public {
        vm.prank(alice);
        faucet.request();
        vm.warp(block.timestamp + 24 hours + 1);
        vm.prank(alice);
        faucet.request();
        assertEq(faucet.requestCount(), 2);
    }

    function test_different_users_independent_cooldowns() public {
        vm.prank(alice);
        faucet.request();
        vm.prank(bob);
        faucet.request(); // bob can request immediately
        assertEq(faucet.requestCount(), 2);
    }

    function test_paused_blocks_requests() public {
        faucet.setPaused(true);
        vm.prank(alice);
        vm.expectRevert();
        faucet.request();
    }

    function test_unpause_restores_requests() public {
        faucet.setPaused(true);
        faucet.setPaused(false);
        vm.prank(alice);
        faucet.request();
        assertEq(alice.balance, 100 ether);
    }

    function test_non_owner_cannot_pause() public {
        vm.prank(alice);
        vm.expectRevert();
        faucet.setPaused(true);
    }

    function test_fund_increases_balance() public {
        uint256 before = address(faucet).balance;
        (bool ok,) = address(faucet).call{value: 1_000 ether}("");
        assertTrue(ok);
        assertEq(address(faucet).balance, before + 1_000 ether);
    }

    function test_empty_faucet_reverts() public {
        // Drain the faucet
        uint256 bal = address(faucet).balance;
        uint256 times = bal / 100 ether;
        for (uint256 i; i < times; i++) {
            address user = address(uint160(i + 1));
            vm.deal(user, 0);
            vm.prank(user);
            faucet.request();
            vm.warp(block.timestamp); // same block is fine for different users
        }
        // Faucet should now be empty or close to empty
        uint256 remaining = address(faucet).balance;
        if (remaining < 100 ether) {
            vm.prank(address(0xDEF));
            vm.expectRevert();
            faucet.request();
        }
    }
}
