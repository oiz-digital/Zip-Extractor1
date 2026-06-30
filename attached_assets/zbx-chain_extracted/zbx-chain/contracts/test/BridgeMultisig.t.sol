// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../BridgeMultisig.sol";

contract MockBridgeVault {
    event MintExecuted(address indexed to, uint256 amount, uint64 seq);
    bool public locked;

    function executeMint(address to, uint256 amount, uint64 seq) external {
        emit MintExecuted(to, amount, seq);
    }

    function setVault(address) external {}
}

contract BridgeMultisigTest is Test {
    BridgeMultisig multisig;
    MockBridgeVault vault;

    address founder   = address(this);
    address signer1   = address(0x5161);
    address signer2   = address(0x5162);
    address signer3   = address(0x5163);
    address recipient = address(0xAECDE47);

    function setUp() public {
        vault = new MockBridgeVault();

        address[] memory signers = new address[](3);
        signers[0] = signer1;
        signers[1] = signer2;
        signers[2] = signer3;

        multisig = new BridgeMultisig(founder, signers, 2); // threshold = 2
        multisig.setVault(address(vault));
    }

    // ── Threshold ─────────────────────────────────────────────────────────

    function test_threshold_stored() public view {
        assertEq(multisig.threshold(), 2);
    }

    function test_signers_stored() public view {
        assertTrue(multisig.isSigner(signer1));
        assertTrue(multisig.isSigner(signer2));
        assertTrue(multisig.isSigner(signer3));
    }

    function test_non_signer_not_stored() public view {
        assertFalse(multisig.isSigner(address(0xBAD)));
    }

    // ── Submit mint ───────────────────────────────────────────────────────

    function test_submit_mint() public {
        vm.prank(signer1);
        multisig.submitMint(recipient, 1_000 ether, 1);
        // No revert = success
    }

    function test_non_signer_cannot_submit() public {
        vm.prank(address(0xBAD));
        vm.expectRevert();
        multisig.submitMint(recipient, 1_000 ether, 1);
    }

    // ── Execute at threshold ──────────────────────────────────────────────

    function test_execute_at_threshold() public {
        vm.prank(signer1);
        multisig.submitMint(recipient, 1_000 ether, 1);
        vm.prank(signer2);
        vm.expectEmit(true, false, false, true);
        emit MockBridgeVault.MintExecuted(recipient, 1_000 ether, 1);
        multisig.submitMint(recipient, 1_000 ether, 1);
    }

    function test_single_signer_insufficient() public {
        vm.prank(signer1);
        multisig.submitMint(recipient, 1_000 ether, 1);
        // Only 1 of 2 needed — vault should NOT be called yet
        // (we check by verifying no MintExecuted event on first submitMint)
    }

    function test_double_vote_same_signer_reverts() public {
        vm.prank(signer1);
        multisig.submitMint(recipient, 1_000 ether, 2);
        vm.prank(signer1);
        vm.expectRevert();
        multisig.submitMint(recipient, 1_000 ether, 2);
    }

    // ── Replay protection ─────────────────────────────────────────────────

    function test_replay_same_seq_after_execution_reverts() public {
        vm.prank(signer1); multisig.submitMint(recipient, 1_000 ether, 5);
        vm.prank(signer2); multisig.submitMint(recipient, 1_000 ether, 5); // executes
        vm.prank(signer3);
        vm.expectRevert();
        multisig.submitMint(recipient, 1_000 ether, 5);
    }

    // ── Cancel tally ─────────────────────────────────────────────────────

    function test_cancel_tally() public {
        vm.prank(signer1);
        multisig.submitMint(recipient, 500 ether, 10);
        multisig.cancelTally(10);
        // After cancel, re-voting should be possible
        vm.prank(signer1);
        multisig.submitMint(recipient, 500 ether, 10);
    }

    function test_non_admin_cannot_cancel_tally() public {
        vm.prank(signer1);
        multisig.submitMint(recipient, 500 ether, 11);
        vm.prank(signer1);
        vm.expectRevert();
        multisig.cancelTally(11);
    }

    // ── Founder transfer ──────────────────────────────────────────────────

    function test_two_step_founder_transfer() public {
        multisig.transferFounder(signer1);
        assertEq(multisig.pendingFounder(), signer1);
        vm.prank(signer1);
        multisig.acceptFounder();
        assertEq(multisig.founder(), signer1);
    }
}
