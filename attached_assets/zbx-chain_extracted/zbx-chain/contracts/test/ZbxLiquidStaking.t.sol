// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../ZbxLiquidStaking.sol";

contract ZbxLiquidStakingTest is Test {
    ZbxLiquidStaking staking;

    address owner    = address(this);
    address alice    = address(0xA11CE);
    address bob      = address(0xB0B);
    address operator = address(0x0PERAT0R);

    function setUp() public {
        staking = new ZbxLiquidStaking();
        staking.setOperator(operator, true);
        vm.deal(alice, 10_000 ether);
        vm.deal(bob, 10_000 ether);
        vm.deal(operator, 1_000 ether);
    }

    // ── Stake ─────────────────────────────────────────────────────────────

    function test_stake_mints_stZbx() public {
        vm.prank(alice);
        staking.stake{value: 100 ether}();
        assertEq(staking.balanceOf(alice), 100 ether);
    }

    function test_stake_increases_total_pool() public {
        vm.prank(alice);
        staking.stake{value: 100 ether}();
        assertEq(staking.totalZbx(), 100 ether);
    }

    function test_stake_zero_reverts() public {
        vm.prank(alice);
        vm.expectRevert();
        staking.stake{value: 0}();
    }

    // ── Exchange rate ─────────────────────────────────────────────────────

    function test_exchange_rate_starts_1_to_1() public {
        vm.prank(alice);
        staking.stake{value: 1_000 ether}();
        // zbxPerShare = totalZbx / totalShares = 1000 / 1000 = 1
        uint256 rate = staking.zbxPerShare();
        assertApproxEqRel(rate, 1e18, 1e15);
    }

    function test_reward_increases_exchange_rate() public {
        vm.prank(alice);
        staking.stake{value: 1_000 ether}();

        // Operator adds rewards
        vm.prank(operator);
        staking.addRewards{value: 100 ether}();

        // Rate should now be > 1
        uint256 rate = staking.zbxPerShare();
        assertGt(rate, 1e18);
    }

    function test_new_staker_after_reward_gets_fewer_shares() public {
        vm.prank(alice);
        staking.stake{value: 1_000 ether}();

        vm.prank(operator);
        staking.addRewards{value: 100 ether}();

        // Bob stakes same amount but gets fewer shares since rate > 1
        vm.prank(bob);
        staking.stake{value: 1_000 ether}();
        assertLt(staking.balanceOf(bob), staking.balanceOf(alice));
    }

    // ── Unstake ───────────────────────────────────────────────────────────

    function test_unstake_burns_shares() public {
        vm.prank(alice);
        staking.stake{value: 1_000 ether}();
        uint256 shares = staking.balanceOf(alice);

        vm.prank(alice);
        staking.unstake(shares / 2);
        assertEq(staking.balanceOf(alice), shares / 2);
    }

    function test_unstake_returns_zbx() public {
        vm.prank(alice);
        staking.stake{value: 1_000 ether}();
        uint256 shares = staking.balanceOf(alice);
        uint256 before = alice.balance;

        vm.prank(alice);
        staking.unstake(shares);
        assertApproxEqRel(alice.balance, before + 1_000 ether, 1e15);
    }

    function test_unstake_more_than_balance_reverts() public {
        vm.prank(alice);
        staking.stake{value: 100 ether}();
        vm.prank(alice);
        vm.expectRevert();
        staking.unstake(200 ether);
    }

    // ── Operator ──────────────────────────────────────────────────────────

    function test_non_operator_cannot_add_rewards() public {
        vm.prank(alice);
        vm.expectRevert();
        staking.addRewards{value: 100 ether}();
    }

    // ── Transfer stZBX ────────────────────────────────────────────────────

    function test_transfer_stZbx() public {
        vm.prank(alice);
        staking.stake{value: 1_000 ether}();
        uint256 shares = staking.balanceOf(alice);

        vm.prank(alice);
        staking.transfer(bob, shares / 2);
        assertEq(staking.balanceOf(bob), shares / 2);
        assertEq(staking.balanceOf(alice), shares / 2);
    }

    // ── Pause ─────────────────────────────────────────────────────────────

    function test_pause_blocks_stake() public {
        staking.pause();
        vm.prank(alice);
        vm.expectRevert();
        staking.stake{value: 100 ether}();
    }
}
