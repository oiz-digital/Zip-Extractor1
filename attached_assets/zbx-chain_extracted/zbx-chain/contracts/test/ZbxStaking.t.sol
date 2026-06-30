// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../ZbxStaking.sol";

contract MockERC20 {
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;
    uint256 public totalSupply;

    function mint(address to, uint256 amount) external {
        balanceOf[to] += amount;
        totalSupply += amount;
    }

    function approve(address spender, uint256 amount) external returns (bool) {
        allowance[msg.sender][spender] = amount;
        return true;
    }

    function transfer(address to, uint256 amount) external returns (bool) {
        require(balanceOf[msg.sender] >= amount, "insufficient");
        balanceOf[msg.sender] -= amount;
        balanceOf[to] += amount;
        return true;
    }

    function transferFrom(address from, address to, uint256 amount) external returns (bool) {
        require(balanceOf[from] >= amount, "insufficient");
        require(allowance[from][msg.sender] >= amount, "not approved");
        allowance[from][msg.sender] -= amount;
        balanceOf[from] -= amount;
        balanceOf[to] += amount;
        return true;
    }
}

contract ZbxStakingTest is Test {
    ZbxStaking staking;
    MockERC20  stakeToken;
    MockERC20  rewardToken;

    address alice = address(0xA11CE);
    address bob   = address(0xB0B);
    address admin = address(this);

    uint256 constant REWARD_RATE = 1e18; // 1 token/sec

    function setUp() public {
        stakeToken  = new MockERC20();
        rewardToken = new MockERC20();

        staking = new ZbxStaking(
            address(stakeToken),
            address(rewardToken),
            REWARD_RATE
        );

        // Fund reward pool
        rewardToken.mint(admin, 1_000_000 ether);
        rewardToken.approve(address(staking), type(uint256).max);
        staking.fundRewards(1_000_000 ether);

        // Give alice and bob tokens
        stakeToken.mint(alice, 10_000 ether);
        stakeToken.mint(bob,   10_000 ether);

        vm.prank(alice);
        stakeToken.approve(address(staking), type(uint256).max);
        vm.prank(bob);
        stakeToken.approve(address(staking), type(uint256).max);
    }

    function test_stake_increases_total_staked() public {
        vm.prank(alice);
        staking.stake(1_000 ether);
        assertEq(staking.totalStaked(), 1_000 ether);
    }

    function test_stake_reduces_user_balance() public {
        vm.prank(alice);
        staking.stake(500 ether);
        assertEq(stakeToken.balanceOf(alice), 9_500 ether);
    }

    function test_unstake_returns_tokens() public {
        vm.prank(alice);
        staking.stake(1_000 ether);
        vm.prank(alice);
        staking.unstake(500 ether);
        assertEq(stakeToken.balanceOf(alice), 9_500 ether);
        assertEq(staking.totalStaked(), 500 ether);
    }

    function test_rewards_accrue_over_time() public {
        vm.prank(alice);
        staking.stake(1_000 ether);

        vm.warp(block.timestamp + 100);

        uint256 pending = staking.pendingReward(alice);
        assertGt(pending, 0);
    }

    function test_claim_transfers_rewards() public {
        vm.prank(alice);
        staking.stake(1_000 ether);
        vm.warp(block.timestamp + 100);

        uint256 before = rewardToken.balanceOf(alice);
        vm.prank(alice);
        staking.claim();
        assertGt(rewardToken.balanceOf(alice), before);
    }

    function test_stake_zero_reverts() public {
        vm.prank(alice);
        vm.expectRevert();
        staking.stake(0);
    }

    function test_unstake_more_than_staked_reverts() public {
        vm.prank(alice);
        staking.stake(100 ether);
        vm.prank(alice);
        vm.expectRevert();
        staking.unstake(200 ether);
    }

    function test_reward_rate_capped() public {
        vm.expectRevert();
        staking.setRewardRate(staking.MAX_REWARD_RATE() + 1);
    }

    function test_two_stakers_proportional_rewards() public {
        vm.prank(alice);
        staking.stake(1_000 ether);
        vm.prank(bob);
        staking.stake(1_000 ether);

        vm.warp(block.timestamp + 1000);

        uint256 alicePending = staking.pendingReward(alice);
        uint256 bobPending   = staking.pendingReward(bob);

        // Both staked equally → rewards should be equal (within rounding)
        assertApproxEqRel(alicePending, bobPending, 1e15); // 0.1% tolerance
    }
}
