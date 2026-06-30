// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../ZRC20Staking.sol";

contract MockStakeToken {
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;
    function mint(address to, uint256 amt) external { balanceOf[to] += amt; }
    function transfer(address to, uint256 amt) external returns (bool) {
        require(balanceOf[msg.sender] >= amt);
        balanceOf[msg.sender] -= amt;
        balanceOf[to] += amt;
        return true;
    }
    function transferFrom(address from, address to, uint256 amt) external returns (bool) {
        require(balanceOf[from] >= amt && allowance[from][msg.sender] >= amt);
        allowance[from][msg.sender] -= amt;
        balanceOf[from] -= amt;
        balanceOf[to] += amt;
        return true;
    }
    function approve(address s, uint256 a) external returns (bool) { allowance[msg.sender][s] = a; return true; }
}

contract ZRC20StakingTest is Test {
    ZRC20Staking staking;
    MockStakeToken stake;
    MockStakeToken reward;

    address owner = address(this);
    address alice = address(0xA11CE);
    address bob   = address(0xB0B);

    function setUp() public {
        stake   = new MockStakeToken();
        reward  = new MockStakeToken();
        staking = new ZRC20Staking(owner, address(stake), address(reward));

        stake.mint(alice, 100_000 ether);
        stake.mint(bob,   100_000 ether);
        reward.mint(address(staking), 1_000_000 ether); // fund rewards

        vm.prank(alice); stake.approve(address(staking), type(uint256).max);
        vm.prank(bob);   stake.approve(address(staking), type(uint256).max);

        // Set reward rate: 1 reward token per second
        staking.setRewardRate(1 ether);
    }

    // ── Stake ────────────────────────────────────────────────────────────

    function test_stake_records_balance() public {
        vm.prank(alice);
        staking.stake(1_000 ether);
        assertEq(staking.balanceOf(alice), 1_000 ether);
    }

    function test_stake_zero_reverts() public {
        vm.prank(alice);
        vm.expectRevert();
        staking.stake(0);
    }

    // ── Earn reward ───────────────────────────────────────────────────────

    function test_earned_increases_over_time() public {
        vm.prank(alice);
        staking.stake(1_000 ether);
        vm.warp(block.timestamp + 100);
        assertGt(staking.earned(alice), 0);
    }

    function test_more_stake_earns_more() public {
        vm.prank(alice);
        staking.stake(2_000 ether);
        vm.prank(bob);
        staking.stake(1_000 ether);
        vm.warp(block.timestamp + 100);
        assertGt(staking.earned(alice), staking.earned(bob));
    }

    // ── Claim reward ─────────────────────────────────────────────────────

    function test_claim_transfers_rewards() public {
        vm.prank(alice);
        staking.stake(1_000 ether);
        vm.warp(block.timestamp + 100);
        uint256 before = reward.balanceOf(alice);
        vm.prank(alice);
        staking.claimReward();
        assertGt(reward.balanceOf(alice), before);
    }

    function test_claim_resets_earned() public {
        vm.prank(alice);
        staking.stake(1_000 ether);
        vm.warp(block.timestamp + 100);
        vm.prank(alice);
        staking.claimReward();
        assertEq(staking.earned(alice), 0);
    }

    // ── Withdraw ─────────────────────────────────────────────────────────

    function test_withdraw_returns_stake() public {
        vm.prank(alice);
        staking.stake(1_000 ether);
        uint256 before = stake.balanceOf(alice);
        vm.prank(alice);
        staking.withdraw(500 ether);
        assertEq(stake.balanceOf(alice), before + 500 ether);
    }

    function test_withdraw_more_than_balance_reverts() public {
        vm.prank(alice);
        staking.stake(1_000 ether);
        vm.prank(alice);
        vm.expectRevert();
        staking.withdraw(2_000 ether);
    }

    // ── Exit ─────────────────────────────────────────────────────────────

    function test_exit_withdraws_and_claims() public {
        vm.prank(alice);
        staking.stake(1_000 ether);
        vm.warp(block.timestamp + 100);
        vm.prank(alice);
        staking.exit();
        assertEq(staking.balanceOf(alice), 0);
        assertGt(reward.balanceOf(alice), 0);
    }

    // ── APR ──────────────────────────────────────────────────────────────

    function test_apr_nonzero_when_staked() public {
        vm.prank(alice);
        staking.stake(1_000 ether);
        assertGt(staking.apr(), 0);
    }

    // ── Emergency withdraw ────────────────────────────────────────────────

    function test_emergency_withdraw_returns_tokens() public {
        vm.prank(alice);
        staking.stake(1_000 ether);
        uint256 before = stake.balanceOf(alice);
        vm.prank(alice);
        staking.emergencyWithdraw();
        assertGt(stake.balanceOf(alice), before);
    }
}
