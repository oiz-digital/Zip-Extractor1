// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../ZRC20Vesting.sol";

contract MockVestToken {
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

contract ZRC20VestingTest is Test {
    ZRC20Vesting vesting;
    MockVestToken token;

    address owner = address(this);
    address alice = address(0xA11CE);
    address bob   = address(0xB0B);

    function setUp() public {
        token   = new MockVestToken();
        vesting = new ZRC20Vesting(owner, address(token));
        token.mint(owner, 10_000_000 ether);
        token.approve(address(vesting), type(uint256).max);
    }

    function _createGrant(address ben, uint256 total, uint64 cliff, uint64 duration) internal {
        vesting.createGrant(ben, total, uint64(block.timestamp), cliff, duration, true);
    }

    // ── Create grant ──────────────────────────────────────────────────────

    function test_create_grant() public {
        _createGrant(alice, 1_000 ether, uint64(30 days), uint64(365 days));
        assertGt(vesting.vested(alice), 0); // might be 0 at cliff
    }

    function test_create_grant_locks_tokens() public {
        _createGrant(alice, 1_000 ether, uint64(30 days), uint64(365 days));
        assertEq(token.balanceOf(address(vesting)), 1_000 ether);
    }

    function test_create_zero_amount_reverts() public {
        vm.expectRevert();
        vesting.createGrant(alice, 0, 0, 0, 0, false);
    }

    function test_create_grant_zero_address_reverts() public {
        vm.expectRevert();
        vesting.createGrant(address(0), 1_000 ether, 0, 0, 365 days, false);
    }

    // ── Release ───────────────────────────────────────────────────────────

    function test_nothing_releasable_before_cliff() public {
        _createGrant(alice, 1_000 ether, uint64(30 days), uint64(365 days));
        vm.prank(alice);
        vesting.release();
        assertEq(token.balanceOf(alice), 0); // before cliff
    }

    function test_releasable_after_cliff() public {
        _createGrant(alice, 1_000 ether, uint64(30 days), uint64(365 days));
        vm.warp(block.timestamp + 30 days + 1);
        uint256 r = vesting.releasable(alice);
        assertGt(r, 0);
    }

    function test_release_transfers_tokens() public {
        _createGrant(alice, 1_200 ether, uint64(0), uint64(120 days));
        vm.warp(block.timestamp + 60 days); // 50% vested
        vm.prank(alice);
        vesting.release();
        // approximately 50% released
        assertApproxEqRel(token.balanceOf(alice), 600 ether, 1e16);
    }

    function test_full_release_after_duration() public {
        _createGrant(alice, 1_000 ether, uint64(0), uint64(100 days));
        vm.warp(block.timestamp + 100 days + 1);
        vm.prank(alice);
        vesting.release();
        assertEq(token.balanceOf(alice), 1_000 ether);
    }

    // ── Revoke ────────────────────────────────────────────────────────────

    function test_revoke_returns_unvested_to_owner() public {
        _createGrant(alice, 1_000 ether, uint64(30 days), uint64(365 days));
        uint256 before = token.balanceOf(owner);
        vesting.revoke(alice);
        assertGt(token.balanceOf(owner), before);
    }

    function test_non_revocable_grant_revoke_reverts() public {
        vesting.createGrant(alice, 1_000 ether, uint64(block.timestamp), 0, 365 days, false);
        vm.expectRevert();
        vesting.revoke(alice);
    }

    function test_non_owner_cannot_revoke() public {
        _createGrant(alice, 1_000 ether, uint64(30 days), uint64(365 days));
        vm.prank(alice);
        vm.expectRevert();
        vesting.revoke(alice);
    }
}
