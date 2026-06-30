// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../ZusdVault.sol";

contract MockCollateral {
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

contract MockZusd {
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

contract MockZusdOracle {
    mapping(address => uint256) private _prices;
    function setPrice(address a, uint256 p) external { _prices[a] = p; }
    function getPrice(address a) external view returns (uint256) { return _prices[a]; }
    function latestRoundData(address a) external view returns (uint80, int256 price, uint256, uint256 updAt, uint80) {
        return (1, int256(_prices[a]), 0, block.timestamp, 1);
    }
}

contract ZusdVaultTest is Test {
    ZusdVault vault;
    MockCollateral collateral;
    MockZusd zusd;
    MockZusdOracle oracle;

    address owner = address(this);
    address alice = address(0xA11CE);
    address bob   = address(0xB0B);

    function setUp() public {
        collateral = new MockCollateral();
        zusd       = new MockZusd();
        oracle     = new MockZusdOracle();

        // ZBX at $0.50
        oracle.setPrice(address(collateral), 5e17);

        vault = new ZusdVault(owner, address(collateral), address(zusd), address(oracle));

        collateral.mint(alice, 100_000 ether);
        collateral.mint(bob, 100_000 ether);

        vm.prank(alice); collateral.approve(address(vault), type(uint256).max);
        vm.prank(bob);   collateral.approve(address(vault), type(uint256).max);
        vm.prank(alice); zusd.approve(address(vault), type(uint256).max);
        vm.prank(bob);   zusd.approve(address(vault), type(uint256).max);
    }

    // ── Open CDP ──────────────────────────────────────────────────────────

    function test_open_cdp() public {
        // Deposit 1000 ZBX ($500) → mint up to $333 ZUSD at 150% CR
        vm.prank(alice);
        vault.openCDP(1_000 ether, 200 ether); // 200 ZUSD (safe)
        assertEq(zusd.balanceOf(alice), 200 ether);
    }

    function test_open_cdp_below_min_cr_reverts() public {
        // 1000 ZBX at $0.50 = $500. 400 ZUSD would be 125% CR < 150% min
        vm.prank(alice);
        vm.expectRevert();
        vault.openCDP(1_000 ether, 400 ether);
    }

    function test_open_cdp_zero_collateral_reverts() public {
        vm.prank(alice);
        vm.expectRevert();
        vault.openCDP(0, 100 ether);
    }

    // ── Deposit collateral ────────────────────────────────────────────────

    function test_deposit_increases_collateral() public {
        vm.prank(alice);
        vault.openCDP(1_000 ether, 100 ether);
        uint256 collBefore = vault.cdpCollateral(alice);
        vm.prank(alice);
        vault.depositCollateral(500 ether);
        assertGt(vault.cdpCollateral(alice), collBefore);
    }

    // ── Repay ZUSD ────────────────────────────────────────────────────────

    function test_repay_reduces_debt() public {
        vm.prank(alice);
        vault.openCDP(1_000 ether, 200 ether);
        uint256 debtBefore = vault.cdpDebt(alice);
        vm.prank(alice);
        vault.repay(100 ether);
        assertLt(vault.cdpDebt(alice), debtBefore);
    }

    // ── Withdraw collateral ───────────────────────────────────────────────

    function test_withdraw_collateral_no_debt() public {
        vm.prank(alice);
        vault.openCDP(1_000 ether, 0);
        uint256 before = collateral.balanceOf(alice);
        vm.prank(alice);
        vault.withdrawCollateral(500 ether);
        assertGt(collateral.balanceOf(alice), before);
    }

    function test_withdraw_leaves_undercollateralised_reverts() public {
        vm.prank(alice);
        vault.openCDP(1_000 ether, 200 ether);
        vm.prank(alice);
        vm.expectRevert();
        vault.withdrawCollateral(900 ether); // would break CR
    }
}
