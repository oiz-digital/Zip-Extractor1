// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../ZbxGameEscrow.sol";

contract MockEscrowToken {
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

contract ZbxGameEscrowTest is Test {
    ZbxGameEscrow  escrow;
    MockEscrowToken token;

    address admin  = address(this);
    address alice  = address(0xA11CE);
    address bob    = address(0xB0B);
    address arbiter = address(0xA4B);

    uint256 constant STAKE = 100 ether;

    function setUp() public {
        token  = new MockEscrowToken();
        escrow = new ZbxGameEscrow(admin, arbiter);

        token.mint(alice, 10_000 ether);
        token.mint(bob,   10_000 ether);

        vm.prank(alice); token.approve(address(escrow), type(uint256).max);
        vm.prank(bob);   token.approve(address(escrow), type(uint256).max);
    }

    function _createMatch() internal returns (bytes32 matchId) {
        vm.prank(alice);
        matchId = escrow.createMatch(address(token), STAKE, bob, block.timestamp + 1 days);
    }

    // ── Create ────────────────────────────────────────────────────────────

    function test_create_match() public {
        bytes32 id = _createMatch();
        assertTrue(id != bytes32(0));
    }

    function test_create_locks_alice_stake() public {
        _createMatch();
        assertEq(token.balanceOf(address(escrow)), STAKE);
    }

    function test_create_zero_stake_reverts() public {
        vm.prank(alice);
        vm.expectRevert();
        escrow.createMatch(address(token), 0, bob, block.timestamp + 1 days);
    }

    // ── Join ──────────────────────────────────────────────────────────────

    function test_join_match() public {
        bytes32 id = _createMatch();
        vm.prank(bob);
        escrow.joinMatch(id);
        assertEq(token.balanceOf(address(escrow)), STAKE * 2);
    }

    function test_non_opponent_cannot_join() public {
        bytes32 id = _createMatch();
        vm.prank(address(0xSTRANGER));
        vm.expectRevert();
        escrow.joinMatch(id);
    }

    function test_join_expired_match_reverts() public {
        bytes32 id = _createMatch();
        vm.warp(block.timestamp + 2 days);
        vm.prank(bob);
        vm.expectRevert();
        escrow.joinMatch(id);
    }

    // ── Resolve ───────────────────────────────────────────────────────────

    function test_arbiter_resolve_winner() public {
        bytes32 id = _createMatch();
        vm.prank(bob);
        escrow.joinMatch(id);

        uint256 before = alice.balance;
        vm.prank(arbiter);
        escrow.resolve(id, alice);
        assertGt(token.balanceOf(alice), 10_000 ether - STAKE); // alice won
    }

    function test_non_arbiter_cannot_resolve() public {
        bytes32 id = _createMatch();
        vm.prank(bob);
        escrow.joinMatch(id);
        vm.prank(alice);
        vm.expectRevert();
        escrow.resolve(id, alice);
    }

    function test_winner_must_be_participant() public {
        bytes32 id = _createMatch();
        vm.prank(bob);
        escrow.joinMatch(id);
        vm.prank(arbiter);
        vm.expectRevert();
        escrow.resolve(id, address(0xSTRANGER));
    }

    // ── Cancel / refund ───────────────────────────────────────────────────

    function test_cancel_unjoined_match_refunds_alice() public {
        bytes32 id = _createMatch();
        vm.warp(block.timestamp + 2 days); // expire
        uint256 before = token.balanceOf(alice);
        vm.prank(alice);
        escrow.cancelExpired(id);
        assertEq(token.balanceOf(alice), before + STAKE);
    }

    function test_cancel_active_match_reverts() public {
        bytes32 id = _createMatch();
        vm.prank(alice);
        vm.expectRevert();
        escrow.cancelExpired(id); // not expired yet
    }
}
