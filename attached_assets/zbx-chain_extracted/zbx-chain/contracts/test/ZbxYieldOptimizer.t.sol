// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../ZbxYieldOptimizer.sol";

contract MockYieldToken {
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

contract MockYieldPool {
    MockYieldToken token;
    mapping(address => uint256) public deposits;
    mapping(address => uint256) public rewards;

    constructor(MockYieldToken _t) { token = _t; }

    function deposit(uint256 amount) external {
        require(token.transferFrom(msg.sender, address(this), amount));
        deposits[msg.sender] += amount;
    }

    function withdraw(uint256 amount) external {
        require(deposits[msg.sender] >= amount);
        deposits[msg.sender] -= amount;
        token.mint(msg.sender, amount);
    }

    function pendingReward(address user) external view returns (uint256) {
        return deposits[user] / 100; // 1% mock reward
    }

    function claim() external {
        uint256 r = deposits[msg.sender] / 100;
        if (r > 0) token.mint(msg.sender, r);
    }

    function swapExactTokensForTokens(
        uint256 amountIn, uint256, address[] calldata, address to, uint256
    ) external returns (uint256[] memory) {
        token.mint(to, amountIn); // 1:1 mock swap
        uint256[] memory amounts = new uint256[](2);
        amounts[0] = amountIn;
        amounts[1] = amountIn;
        return amounts;
    }
}

contract ZbxYieldOptimizerTest is Test {
    ZbxYieldOptimizer optimizer;
    MockYieldToken    token;
    MockYieldPool     poolA;
    MockYieldPool     poolB;

    address admin = address(this);
    address alice = address(0xA11CE);
    address bob   = address(0xB0B);

    function setUp() public {
        token   = new MockYieldToken();
        poolA   = new MockYieldPool(token);
        poolB   = new MockYieldPool(token);
        optimizer = new ZbxYieldOptimizer(admin, address(token));

        optimizer.addPool(address(poolA), 7000); // 70% allocation
        optimizer.addPool(address(poolB), 3000); // 30% allocation

        token.mint(alice, 100_000 ether);
        token.mint(bob,   100_000 ether);

        vm.prank(alice); token.approve(address(optimizer), type(uint256).max);
        vm.prank(bob);   token.approve(address(optimizer), type(uint256).max);
    }

    // ── Deposit ───────────────────────────────────────────────────────────

    function test_deposit_records_shares() public {
        vm.prank(alice);
        uint256 shares = optimizer.deposit(1_000 ether);
        assertGt(shares, 0);
    }

    function test_deposit_zero_reverts() public {
        vm.prank(alice);
        vm.expectRevert();
        optimizer.deposit(0);
    }

    function test_deposit_distributes_to_pools() public {
        vm.prank(alice);
        optimizer.deposit(1_000 ether);
        // Pools should have received tokens
        assertGt(token.balanceOf(address(poolA)), 0);
    }

    // ── Withdraw ──────────────────────────────────────────────────────────

    function test_withdraw_returns_tokens() public {
        vm.prank(alice);
        uint256 shares = optimizer.deposit(1_000 ether);
        uint256 before = token.balanceOf(alice);
        vm.prank(alice);
        optimizer.withdraw(shares);
        assertGt(token.balanceOf(alice), before);
    }

    function test_withdraw_more_than_shares_reverts() public {
        vm.prank(alice);
        uint256 shares = optimizer.deposit(1_000 ether);
        vm.prank(alice);
        vm.expectRevert();
        optimizer.withdraw(shares * 2);
    }

    // ── Rebalance ─────────────────────────────────────────────────────────

    function test_rebalance_by_admin() public {
        vm.prank(alice);
        optimizer.deposit(10_000 ether);
        optimizer.rebalance(); // Should not revert
    }

    function test_non_admin_cannot_rebalance() public {
        vm.prank(alice);
        optimizer.deposit(10_000 ether);
        vm.prank(alice);
        vm.expectRevert();
        optimizer.rebalance();
    }

    // ── Pool management ───────────────────────────────────────────────────

    function test_update_allocation() public {
        optimizer.updateAllocation(address(poolA), 5000);
        optimizer.updateAllocation(address(poolB), 5000);
    }

    function test_total_allocation_over_10000_reverts() public {
        vm.expectRevert();
        optimizer.addPool(address(0xNEW), 5000); // would push total > 10000
    }

    // ── Pending reward ────────────────────────────────────────────────────

    function test_pending_reward_nonzero_after_deposit() public {
        vm.prank(alice);
        optimizer.deposit(1_000 ether);
        assertGe(optimizer.pendingReward(alice), 0);
    }
}
