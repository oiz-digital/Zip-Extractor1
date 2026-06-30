// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../RewardPool.sol";

contract MockStaking {
    uint256 public totalStaked;
    mapping(address => uint256) private _staked;

    function setTotalStaked(uint256 amt) external { totalStaked = amt; }
    function setStaked(address v, uint256 amt) external { _staked[v] = amt; totalStaked += amt; }
    function stakedBy(address v) external view returns (uint256) { return _staked[v]; }
}

contract RewardPoolTest is Test {
    RewardPool pool;
    MockStaking staking;

    address owner     = address(this);
    address validator1 = address(0xV1);
    address validator2 = address(0xV2);

    function setUp() public {
        staking = new MockStaking();
        pool    = new RewardPool{value: 10_000 ether}(address(staking));

        // Give validators staked amounts
        staking.setStaked(validator1, 600_000 ether); // 60%
        staking.setStaked(validator2, 400_000 ether); // 40%
    }

    // ── Epoch settlement ──────────────────────────────────────────────────

    function test_settle_epoch_increments_epoch() public {
        pool.settleEpoch{value: 100 ether}(100 ether);
        assertEq(pool.currentEpoch(), 1);
    }

    function test_settle_epoch_updates_reward_per_token() public {
        pool.settleEpoch{value: 100 ether}(100 ether);
        assertGt(pool.rewardPerToken(), 0);
    }

    function test_epoch_emission_cap_enforced() public {
        uint256 cap = pool.EPOCH_EMISSION_CAP();
        vm.expectRevert();
        pool.settleEpoch{value: cap + 1}(cap + 1);
    }

    // ── Pending reward ────────────────────────────────────────────────────

    function test_pending_reward_after_epoch() public {
        pool.settleEpoch{value: 1_000 ether}(1_000 ether);
        uint256 pending = pool.pendingReward(validator1);
        assertGt(pending, 0);
    }

    function test_reward_proportional_to_stake() public {
        pool.settleEpoch{value: 1_000 ether}(1_000 ether);
        uint256 p1 = pool.pendingReward(validator1);
        uint256 p2 = pool.pendingReward(validator2);
        // validator1 has 60%, validator2 has 40%
        assertGt(p1, p2);
        // Approximate ratio: p1/p2 ≈ 60/40 = 1.5
        assertApproxEqRel(p1, (p2 * 3) / 2, 1e15);
    }

    function test_no_stake_no_reward() public {
        pool.settleEpoch{value: 1_000 ether}(1_000 ether);
        assertEq(pool.pendingReward(address(0xNEW)), 0);
    }

    // ── Claim ─────────────────────────────────────────────────────────────

    function test_claim_transfers_native_zbx() public {
        pool.settleEpoch{value: 1_000 ether}(1_000 ether);
        uint256 before = validator1.balance;
        vm.prank(validator1);
        uint256 claimed = pool.claimReward();
        assertGt(claimed, 0);
        assertEq(validator1.balance, before + claimed);
    }

    function test_double_claim_returns_zero() public {
        pool.settleEpoch{value: 1_000 ether}(1_000 ether);
        vm.prank(validator1);
        pool.claimReward();
        vm.prank(validator1);
        uint256 second = pool.claimReward();
        assertEq(second, 0);
    }

    function test_claim_resets_pending() public {
        pool.settleEpoch{value: 1_000 ether}(1_000 ether);
        vm.prank(validator1);
        pool.claimReward();
        assertEq(pool.pendingReward(validator1), 0);
    }

    // ── APR estimate ──────────────────────────────────────────────────────

    function test_estimated_apr_nonzero() public view {
        uint256 apr = pool.estimatedAprBps(1_000 ether);
        assertGt(apr, 0);
    }

    // ── Blocks until next epoch ───────────────────────────────────────────

    function test_blocks_until_next_epoch_within_epoch() public view {
        uint256 remaining = pool.blocksUntilNextEpoch();
        assertLe(remaining, pool.EPOCH_BLOCKS());
    }
}
