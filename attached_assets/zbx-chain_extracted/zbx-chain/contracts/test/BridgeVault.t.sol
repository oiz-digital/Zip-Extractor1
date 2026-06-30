// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../BridgeVault.sol";

/// Minimal mock ZRC20 for BridgeVault
contract MockZRC20 {
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;
    uint256 public totalSupply;

    address public vault;

    function setVault(address v) external { vault = v; }

    function mint(address to, uint256 amt) external {
        balanceOf[to] += amt;
        totalSupply += amt;
    }

    function burn(address from, uint256 amt) external {
        require(balanceOf[from] >= amt, "burn: insufficient");
        balanceOf[from] -= amt;
        totalSupply -= amt;
    }

    function transfer(address to, uint256 amt) external returns (bool) {
        require(balanceOf[msg.sender] >= amt, "insufficient");
        balanceOf[msg.sender] -= amt;
        balanceOf[to] += amt;
        return true;
    }

    function transferFrom(address from, address to, uint256 amt) external returns (bool) {
        require(balanceOf[from] >= amt, "insufficient");
        require(allowance[from][msg.sender] >= amt, "not approved");
        allowance[from][msg.sender] -= amt;
        balanceOf[from] -= amt;
        balanceOf[to] += amt;
        return true;
    }

    function approve(address spender, uint256 amt) external returns (bool) {
        allowance[msg.sender][spender] = amt;
        return true;
    }

    function bridgeMint(address to, uint256 amt, uint64) external {
        require(msg.sender == vault, "not vault");
        balanceOf[to] += amt;
        totalSupply += amt;
    }

    function bridgeBurnFrom(address from, uint256 amt) external {
        require(msg.sender == vault, "not vault");
        require(balanceOf[from] >= amt);
        balanceOf[from] -= amt;
        totalSupply -= amt;
    }
}

contract MockMultisig {
    address public vault;
    function setVault(address v) external { vault = v; }
}

contract BridgeVaultTest is Test {
    BridgeVault vault;
    MockZRC20   token;
    MockMultisig multisig;

    address founder  = address(this);
    address alice    = address(0xA11CE);
    address relayer  = address(0xBABE);

    function setUp() public {
        token    = new MockZRC20();
        multisig = new MockMultisig();
        vault    = new BridgeVault(address(token), address(multisig));
        token.setVault(address(vault));
        multisig.setVault(address(vault));

        // Fund alice
        token.mint(alice, 100_000 ether);
        vm.prank(alice);
        token.approve(address(vault), type(uint256).max);
    }

    // ── Basic state ──────────────────────────────────────────────────────

    function test_token_and_multisig_set() public view {
        assertEq(vault.token(), address(token));
        assertEq(vault.multisig(), address(multisig));
    }

    // ── Lock (bridge to ZBX mainnet) ─────────────────────────────────────

    function test_lock_burns_tokens() public {
        uint256 before = token.totalSupply();
        vm.prank(alice);
        vault.lock(1_000 ether, alice);
        assertEq(token.totalSupply(), before - 1_000 ether);
    }

    function test_lock_emits_sequence_number() public {
        vm.prank(alice);
        uint64 seq = vault.lock(1_000 ether, alice);
        assertGt(seq, 0);
    }

    function test_lock_zero_reverts() public {
        vm.prank(alice);
        vm.expectRevert();
        vault.lock(0, alice);
    }

    function test_lock_insufficient_balance_reverts() public {
        address poorUser = address(0xPOOR);
        vm.prank(poorUser);
        vm.expectRevert();
        vault.lock(1_000 ether, poorUser);
    }

    // ── Mint (bridge from ZBX mainnet) ───────────────────────────────────

    function test_execute_mint_from_multisig() public {
        // Simulate multisig calling executeMint
        vm.prank(address(multisig));
        vault.executeMint(alice, 500 ether, 1);
        assertEq(token.balanceOf(alice), 100_000 ether + 500 ether);
    }

    function test_replay_protection() public {
        vm.prank(address(multisig));
        vault.executeMint(alice, 500 ether, 1);
        vm.prank(address(multisig));
        vm.expectRevert();
        vault.executeMint(alice, 500 ether, 1); // same seq → replay
    }

    function test_non_multisig_cannot_execute_mint() public {
        vm.prank(alice);
        vm.expectRevert();
        vault.executeMint(alice, 500 ether, 99);
    }

    // ── Pause ─────────────────────────────────────────────────────────────

    function test_pause_blocks_lock() public {
        vault.pause();
        vm.prank(alice);
        vm.expectRevert();
        vault.lock(1_000 ether, alice);
    }

    // ── Founder transfer ──────────────────────────────────────────────────

    function test_two_step_ownership() public {
        vault.transferFounder(alice);
        assertEq(vault.pendingFounder(), alice);
        vm.prank(alice);
        vault.acceptFounder();
        assertEq(vault.founder(), alice);
    }
}
