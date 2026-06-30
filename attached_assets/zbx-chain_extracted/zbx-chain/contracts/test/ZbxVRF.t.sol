// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../ZbxVRF.sol";

contract ZbxVRFTest is Test {
    ZbxVRF vrf;
    address admin = address(this);
    address requester = address(0xA11CE);

    function setUp() public {
        vrf = new ZbxVRF(admin);
    }

    // ── Request randomness ────────────────────────────────────────────────

    function test_request_random_returns_id() public {
        vm.prank(requester);
        bytes32 id = vrf.requestRandom(keccak256("seed_commitment"));
        assertTrue(id != bytes32(0));
    }

    function test_request_random_different_seeds_different_ids() public {
        vm.prank(requester);
        bytes32 id1 = vrf.requestRandom(keccak256("seed1"));
        vm.prank(requester);
        bytes32 id2 = vrf.requestRandom(keccak256("seed2"));
        assertTrue(id1 != id2);
    }

    // ── Reveal / fulfill ─────────────────────────────────────────────────

    function test_is_revealable_after_delay() public {
        vm.prank(requester);
        bytes32 id = vrf.requestRandom(keccak256("seed"));
        // Should not be revealable immediately
        assertFalse(vrf.isRevealable(id));
        // Advance past reveal delay
        vm.roll(block.number + vrf.REVEAL_DELAY() + 1);
        assertTrue(vrf.isRevealable(id));
    }

    function test_fulfill_returns_nonzero_randomness() public {
        vm.prank(requester);
        bytes32 seed = keccak256("myseed");
        bytes32 seedHash = keccak256(abi.encodePacked(seed));
        bytes32 id = vrf.requestRandom(seedHash);
        vm.roll(block.number + vrf.REVEAL_DELAY() + 1);
        uint256 rand = vrf.fulfillRandom(id, seed);
        assertGt(rand, 0);
    }

    function test_fulfill_before_delay_reverts() public {
        vm.prank(requester);
        bytes32 seed = keccak256("myseed");
        bytes32 id = vrf.requestRandom(keccak256(abi.encodePacked(seed)));
        vm.expectRevert();
        vrf.fulfillRandom(id, seed);
    }

    function test_wrong_seed_reverts() public {
        vm.prank(requester);
        bytes32 seed = keccak256("myseed");
        bytes32 id = vrf.requestRandom(keccak256(abi.encodePacked(seed)));
        vm.roll(block.number + vrf.REVEAL_DELAY() + 1);
        vm.expectRevert();
        vrf.fulfillRandom(id, keccak256("wrongseed"));
    }

    function test_double_fulfill_reverts() public {
        vm.prank(requester);
        bytes32 seed = keccak256("s");
        bytes32 id = vrf.requestRandom(keccak256(abi.encodePacked(seed)));
        vm.roll(block.number + vrf.REVEAL_DELAY() + 1);
        vrf.fulfillRandom(id, seed);
        vm.expectRevert();
        vrf.fulfillRandom(id, seed);
    }

    // ── Combined randomness ───────────────────────────────────────────────

    function test_combined_random_deterministic() public view {
        bytes32 s0 = keccak256("player1_seed");
        bytes32 s1 = keccak256("player2_seed");
        bytes32 nonce = keccak256("gameId");
        uint256 r1 = vrf.combinedRandom(s0, s1, nonce);
        uint256 r2 = vrf.combinedRandom(s0, s1, nonce);
        assertEq(r1, r2);
    }

    function test_combined_random_different_seeds_different_outputs() public view {
        bytes32 nonce = keccak256("nonce");
        uint256 r1 = vrf.combinedRandom(keccak256("a"), keccak256("b"), nonce);
        uint256 r2 = vrf.combinedRandom(keccak256("c"), keccak256("d"), nonce);
        assertTrue(r1 != r2);
    }

    // ── Blocks until expiry ───────────────────────────────────────────────

    function test_blocks_until_expiry_decreases() public {
        vm.prank(requester);
        bytes32 id = vrf.requestRandom(keccak256("s"));
        uint256 b1 = vrf.blocksUntilExpiry(id);
        vm.roll(block.number + 10);
        uint256 b2 = vrf.blocksUntilExpiry(id);
        assertLt(b2, b1);
    }
}
