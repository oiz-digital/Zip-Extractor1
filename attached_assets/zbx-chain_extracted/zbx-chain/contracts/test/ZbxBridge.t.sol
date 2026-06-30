// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../ZbxBridge.sol";

contract MockToken {
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    function mint(address to, uint256 amount) external {
        balanceOf[to] += amount;
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
        require(balanceOf[from] >= amount);
        require(allowance[from][msg.sender] >= amount);
        allowance[from][msg.sender] -= amount;
        balanceOf[from] -= amount;
        balanceOf[to] += amount;
        return true;
    }
}

contract ZbxBridgeTest is Test {
    ZbxBridge bridge;
    MockToken token;

    address admin = address(this);
    address alice = address(0xA11CE);
    address relay1 = address(0xBABE1);
    address relay2 = address(0xBABE2);
    address relay3 = address(0xBABE3);

    address[] relays;

    function setUp() public {
        relays = [relay1, relay2, relay3];
        bridge = new ZbxBridge(admin, relays, 2);
        token  = new MockToken();
        token.mint(alice, 100_000 ether);
        vm.prank(alice);
        token.approve(address(bridge), type(uint256).max);

        // Register token on bridge
        bridge.registerToken(address(token), 1e16, 100_000 ether);
    }

    // ── Basic state ──────────────────────────────────────────────────────

    function test_threshold_is_2() public view {
        assertEq(bridge.threshold(), 2);
    }

    function test_threshold_below_2_reverts() public {
        vm.expectRevert();
        new ZbxBridge(admin, relays, 1);
    }

    function test_not_paused_initially() public view {
        assertFalse(bridge.paused());
    }

    // ── Pause ────────────────────────────────────────────────────────────

    function test_pause_and_unpause() public {
        bridge.pause();
        assertTrue(bridge.paused());
        bridge.unpause();
        assertFalse(bridge.paused());
    }

    function test_non_admin_cannot_pause() public {
        vm.prank(alice);
        vm.expectRevert();
        bridge.pause();
    }

    // ── Bridge out ────────────────────────────────────────────────────────

    function test_bridge_out_locks_tokens() public {
        uint256 amount = 1_000 ether;
        uint256 fee = bridge.bridgeFee(address(token), amount);
        uint256 total = amount + fee;

        token.mint(alice, fee);
        vm.prank(alice);
        token.approve(address(bridge), type(uint256).max);

        vm.prank(alice);
        bridge.bridgeOut(address(token), amount, 1, alice);

        // Tokens should be in bridge
        assertEq(token.balanceOf(address(bridge)), total);
    }

    function test_bridge_out_below_minimum_reverts() public {
        vm.prank(alice);
        vm.expectRevert();
        bridge.bridgeOut(address(token), 1 wei, 1, alice);
    }

    function test_bridge_out_paused_reverts() public {
        bridge.pause();
        vm.prank(alice);
        vm.expectRevert();
        bridge.bridgeOut(address(token), 1_000 ether, 1, alice);
    }

    // ── Relay signatures ─────────────────────────────────────────────────

    function test_is_relay() public view {
        assertTrue(bridge.isRelay(relay1));
        assertTrue(bridge.isRelay(relay2));
        assertFalse(bridge.isRelay(alice));
    }

    function test_non_relay_cannot_submit_signatures() public {
        vm.prank(alice);
        vm.expectRevert();
        bridge.relaySignature(bytes32(0), alice);
    }

    // ── Emergency withdraw ────────────────────────────────────────────────

    function test_emergency_withdraw_by_admin() public {
        token.mint(address(bridge), 5_000 ether);
        uint256 before = token.balanceOf(admin);
        bridge.emergencyWithdraw(address(token), 5_000 ether, admin);
        assertEq(token.balanceOf(admin), before + 5_000 ether);
    }

    function test_emergency_withdraw_non_admin_reverts() public {
        vm.prank(alice);
        vm.expectRevert();
        bridge.emergencyWithdraw(address(token), 1 ether, alice);
    }
}
