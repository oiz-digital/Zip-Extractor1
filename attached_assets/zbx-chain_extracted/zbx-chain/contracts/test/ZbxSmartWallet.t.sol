// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../ZbxSmartWallet.sol";

contract ZbxSmartWalletTest is Test {
    ZbxSmartWallet wallet;

    address owner     = address(0x0WNER);
    address entryPoint = address(0xEE);
    address sessionKey = address(0x5E5510N);
    address guardian1  = address(0x6UA4D1);
    address guardian2  = address(0x6UA4D2);
    address target     = address(0x7A467E7);

    function setUp() public {
        wallet = new ZbxSmartWallet(owner, entryPoint);
        vm.deal(address(wallet), 10 ether);
        vm.deal(owner, 10 ether);
    }

    // ── Execute ───────────────────────────────────────────────────────────

    function test_owner_can_execute() public {
        uint256 before = target.balance;
        vm.prank(entryPoint);
        wallet.execute(target, 1 ether, "");
        assertEq(target.balance, before + 1 ether);
    }

    function test_non_owner_cannot_execute() public {
        vm.prank(address(0xEVIL));
        vm.expectRevert();
        wallet.execute(target, 0.1 ether, "");
    }

    // ── Batch execute ─────────────────────────────────────────────────────

    function test_execute_batch() public {
        address[] memory targets = new address[](2);
        uint256[] memory values  = new uint256[](2);
        bytes[]   memory datas   = new bytes[](2);
        targets[0] = target;
        targets[1] = target;
        values[0]  = 0.5 ether;
        values[1]  = 0.5 ether;
        datas[0]   = "";
        datas[1]   = "";

        uint256 before = target.balance;
        vm.prank(entryPoint);
        wallet.executeBatch(targets, values, datas);
        assertEq(target.balance, before + 1 ether);
    }

    // ── Session keys ──────────────────────────────────────────────────────

    function test_add_session_key() public {
        vm.prank(entryPoint);
        wallet.addSessionKey(
            sessionKey,
            target,
            block.timestamp + 1 days,
            1 ether
        );
        assertTrue(wallet.isSessionKeyValid(sessionKey));
    }

    function test_expired_session_key_invalid() public {
        vm.prank(entryPoint);
        wallet.addSessionKey(sessionKey, target, block.timestamp + 1 days, 1 ether);
        vm.warp(block.timestamp + 2 days);
        assertFalse(wallet.isSessionKeyValid(sessionKey));
    }

    function test_revoke_session_key() public {
        vm.prank(entryPoint);
        wallet.addSessionKey(sessionKey, target, block.timestamp + 1 days, 1 ether);
        vm.prank(entryPoint);
        wallet.revokeSessionKey(sessionKey);
        assertFalse(wallet.isSessionKeyValid(sessionKey));
    }

    // ── Social recovery ───────────────────────────────────────────────────

    function test_add_guardian() public {
        vm.prank(entryPoint);
        wallet.addGuardian(guardian1);
        assertTrue(wallet.isGuardian(guardian1));
    }

    function test_recovery_requires_threshold() public {
        vm.prank(entryPoint);
        wallet.addGuardian(guardian1);
        vm.prank(entryPoint);
        wallet.addGuardian(guardian2);
        vm.prank(entryPoint);
        wallet.setRecoveryThreshold(2);

        address newOwner = address(0xNEW0WNER);
        vm.prank(guardian1);
        wallet.executeRecovery(newOwner);
        // 1 guardian not enough — owner unchanged (threshold = 2)
        assertEq(wallet.owner(), owner);
    }

    function test_recovery_succeeds_at_threshold() public {
        vm.prank(entryPoint);
        wallet.addGuardian(guardian1);
        vm.prank(entryPoint);
        wallet.addGuardian(guardian2);
        vm.prank(entryPoint);
        wallet.setRecoveryThreshold(2);

        address newOwner = address(0xNEW0WNER);
        vm.prank(guardian1);
        wallet.executeRecovery(newOwner);
        vm.prank(guardian2);
        wallet.executeRecovery(newOwner);
        assertEq(wallet.owner(), newOwner);
    }

    // ── Receive ETH ───────────────────────────────────────────────────────

    function test_receive_eth() public {
        uint256 before = address(wallet).balance;
        (bool ok,) = address(wallet).call{value: 1 ether}("");
        assertTrue(ok);
        assertEq(address(wallet).balance, before + 1 ether);
    }
}
