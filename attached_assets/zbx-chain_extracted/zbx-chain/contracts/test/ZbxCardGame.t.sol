// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../ZbxCardGame.sol";

contract MockGameVRF {
    mapping(bytes32 => bool)    public revealable;
    mapping(bytes32 => uint256) public results;

    function requestRandom(bytes32) external returns (bytes32 requestId) {
        requestId = keccak256(abi.encodePacked(block.timestamp, gasleft()));
        revealable[requestId] = true;
        results[requestId] = uint256(requestId);
        return requestId;
    }
    function fulfillRandom(bytes32 requestId, bytes32) external returns (uint256) {
        return results[requestId];
    }
    function isRevealable(bytes32 requestId) external view returns (bool) {
        return revealable[requestId];
    }
}

contract MockGameToken {
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
    function approve(address s, uint256 a) external returns (bool) {
        allowance[msg.sender][s] = a;
        return true;
    }
}

contract ZbxCardGameTest is Test {
    ZbxCardGame game;
    MockGameVRF vrf;
    MockGameToken token;

    address admin = address(this);
    address alice = address(0xA11CE);
    address bob   = address(0xB0B);

    uint256 constant STAKE = 10 ether;

    function setUp() public {
        vrf   = new MockGameVRF();
        token = new MockGameToken();
        game  = new ZbxCardGame(admin, address(vrf), address(token));

        token.mint(alice, 1_000 ether);
        token.mint(bob,   1_000 ether);

        vm.prank(alice); token.approve(address(game), type(uint256).max);
        vm.prank(bob);   token.approve(address(game), type(uint256).max);
    }

    // ── Room creation ─────────────────────────────────────────────────────

    function test_create_room() public {
        vm.prank(alice);
        uint256 roomId = game.createRoom(STAKE, 2);
        assertGt(roomId, 0);
    }

    function test_create_room_zero_stake_reverts() public {
        vm.prank(alice);
        vm.expectRevert();
        game.createRoom(0, 2);
    }

    // ── Join room ────────────────────────────────────────────────────────

    function test_join_room() public {
        vm.prank(alice);
        uint256 roomId = game.createRoom(STAKE, 2);
        vm.prank(bob);
        game.joinRoom(roomId);
        assertEq(game.playerCount(roomId), 2);
    }

    function test_join_full_room_reverts() public {
        vm.prank(alice);
        uint256 roomId = game.createRoom(STAKE, 2);
        vm.prank(bob);
        game.joinRoom(roomId);
        address carol = address(0xCA401);
        token.mint(carol, 1_000 ether);
        vm.prank(carol); token.approve(address(game), type(uint256).max);
        vm.prank(carol);
        vm.expectRevert();
        game.joinRoom(roomId);
    }

    function test_double_join_reverts() public {
        vm.prank(alice);
        uint256 roomId = game.createRoom(STAKE, 2);
        vm.prank(alice);
        vm.expectRevert();
        game.joinRoom(roomId);
    }

    // ── Commit-reveal ─────────────────────────────────────────────────────

    function test_commit_seed() public {
        vm.prank(alice);
        uint256 roomId = game.createRoom(STAKE, 2);
        vm.prank(bob);
        game.joinRoom(roomId);
        bytes32 seedHash = keccak256(abi.encodePacked("alice_seed"));
        vm.prank(alice);
        game.commitSeed(roomId, seedHash);
        // No revert = success
    }

    function test_reveal_seed() public {
        vm.prank(alice);
        uint256 roomId = game.createRoom(STAKE, 2);
        vm.prank(bob);
        game.joinRoom(roomId);

        bytes32 seed = keccak256(abi.encodePacked("alice_seed"));
        bytes32 seedHash = keccak256(abi.encodePacked(seed));

        vm.prank(alice); game.commitSeed(roomId, seedHash);
        vm.prank(bob);   game.commitSeed(roomId, keccak256(abi.encodePacked(seed)));
        vm.prank(alice); game.revealSeed(roomId, seed);
    }

    // ── Stakes ────────────────────────────────────────────────────────────

    function test_stake_locked_on_join() public {
        vm.prank(alice);
        uint256 roomId = game.createRoom(STAKE, 2);
        assertEq(token.balanceOf(address(game)), STAKE);
        vm.prank(bob);
        game.joinRoom(roomId);
        assertEq(token.balanceOf(address(game)), STAKE * 2);
    }
}
