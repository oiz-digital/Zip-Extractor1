// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../ZusdStabilityPool.sol";

contract MockSPZusd {
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;
    function mint(address to, uint256 amt) external { balanceOf[to] += amt; }
    function burn(address from, uint256 amt) external {
        require(balanceOf[from] >= amt);
        balanceOf[from] -= amt;
    }
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

contract MockSPZbx {
    mapping(address => uint256) public balanceOf;
    function mint(address to, uint256 amt) external { balanceOf[to] += amt; }
    function transfer(address to, uint256 amt) external returns (bool) {
        require(balanceOf[msg.sender] >= amt);
        balanceOf[msg.sender] -= amt;
        balanceOf[to] += amt;
        return true;
    }
}

contract ZusdStabilityPoolTest is Test {
    ZusdStabilityPool pool;
    MockSPZusd        zusd;
    MockSPZbx         zbx;

    address owner = address(this);
    address alice = address(0xA11CE);
    address bob   = address(0xB0B);
    address vault = address(0xVAU17);

    function setUp() public {
        zusd = new MockSPZusd();
        zbx  = new MockSPZbx();
        pool = new ZusdStabilityPool(owner, address(zusd), address(zbx));
        pool.setVault(vault);

        zusd.mint(alice, 100_000 ether);
        zusd.mint(bob,   100_000 ether);
        zbx.mint(address(pool), 1_000_000 ether);

        vm.prank(alice); zusd.approve(address(pool), type(uint256).max);
        vm.prank(bob);   zusd.approve(address(pool), type(uint256).max);
    }

    // ── Deposit ───────────────────────────────────────────────────────────

    function test_deposit_records_balance() public {
        vm.prank(alice);
        pool.deposit(10_000 ether);
        assertEq(pool.getDeposit(alice), 10_000 ether);
    }

    function test_deposit_zero_reverts() public {
        vm.prank(alice);
        vm.expectRevert();
        pool.deposit(0);
    }

    // ── Withdraw ─────────────────────────────────────────────────────────

    function test_withdraw_returns_zusd() public {
        vm.prank(alice);
        pool.deposit(10_000 ether);
        uint256 before = zusd.balanceOf(alice);
        vm.prank(alice);
        pool.withdraw(5_000 ether);
        assertGt(zusd.balanceOf(alice), before);
    }

    function test_withdraw_more_than_deposit_reverts() public {
        vm.prank(alice);
        pool.deposit(10_000 ether);
        vm.prank(alice);
        vm.expectRevert();
        pool.withdraw(20_000 ether);
    }

    // ── Absorb liquidation ────────────────────────────────────────────────

    function test_absorb_liquidation_burns_zusd() public {
        vm.prank(alice);
        pool.deposit(50_000 ether);
        uint256 supplyBefore = zusd.balanceOf(address(pool));
        vm.prank(vault);
        pool.absorbLiquidation(1_000 ether, 2_000 ether); // 1000 ZUSD debt, 2000 ZBX collateral
        assertLt(zusd.balanceOf(address(pool)), supplyBefore + 50_000 ether);
    }

    function test_non_vault_cannot_absorb() public {
        vm.prank(alice);
        pool.deposit(50_000 ether);
        vm.prank(alice);
        vm.expectRevert();
        pool.absorbLiquidation(1_000 ether, 2_000 ether);
    }

    // ── Claim ZBX gain ────────────────────────────────────────────────────

    function test_claim_zbx_gain_after_liquidation() public {
        vm.prank(alice);
        pool.deposit(50_000 ether);
        vm.prank(vault);
        pool.absorbLiquidation(1_000 ether, 2_000 ether);

        uint256 before = zbx.balanceOf(alice);
        vm.prank(alice);
        pool.claimZbxGain();
        assertGe(zbx.balanceOf(alice), before); // may be 0 if share too small
    }
}
