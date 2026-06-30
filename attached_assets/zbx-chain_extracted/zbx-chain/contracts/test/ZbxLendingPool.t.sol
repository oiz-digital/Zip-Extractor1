// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../ZbxLendingPool.sol";

contract MockAsset {
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
    function decimals() external pure returns (uint8) { return 18; }
}

contract MockOracle {
    mapping(address => uint256) private _prices;
    function setPrice(address asset, uint256 price) external { _prices[asset] = price; }
    function getPrice(address asset) external view returns (uint256) { return _prices[asset]; }
}

contract ZbxLendingPoolTest is Test {
    ZbxLendingPool pool;
    MockAsset      collateralToken;
    MockAsset      borrowToken;
    MockOracle     oracle;

    address admin = address(this);
    address alice = address(0xA11CE);
    address bob   = address(0xB0B);

    function setUp() public {
        collateralToken = new MockAsset();
        borrowToken     = new MockAsset();
        oracle          = new MockOracle();

        pool = new ZbxLendingPool(admin, address(oracle));

        // Register reserves
        pool.addReserve(address(collateralToken), 7500); // 75% LTV
        pool.addReserve(address(borrowToken),     8000); // 80% LTV

        // Set prices: 1 collateral = $1000, 1 borrow = $1
        oracle.setPrice(address(collateralToken), 1_000 * 1e18);
        oracle.setPrice(address(borrowToken),     1 * 1e18);

        // Fund alice and bob
        collateralToken.mint(alice, 100 ether);
        borrowToken.mint(alice, 100_000 ether);
        borrowToken.mint(address(pool), 1_000_000 ether); // pool liquidity

        vm.prank(alice);
        collateralToken.approve(address(pool), type(uint256).max);
        vm.prank(alice);
        borrowToken.approve(address(pool), type(uint256).max);
    }

    // ── Supply ────────────────────────────────────────────────────────────

    function test_supply_records_balance() public {
        vm.prank(alice);
        pool.supply(address(collateralToken), 10 ether);
        assertGt(pool.userCollateral(alice, address(collateralToken)), 0);
    }

    function test_supply_zero_reverts() public {
        vm.prank(alice);
        vm.expectRevert();
        pool.supply(address(collateralToken), 0);
    }

    function test_supply_unregistered_asset_reverts() public {
        MockAsset fake = new MockAsset();
        vm.prank(alice);
        vm.expectRevert();
        pool.supply(address(fake), 10 ether);
    }

    // ── Borrow ────────────────────────────────────────────────────────────

    function test_borrow_within_ltv() public {
        vm.prank(alice);
        pool.supply(address(collateralToken), 10 ether); // $10,000 collateral
        // Can borrow up to 75% = $7,500 → 7500 borrow tokens
        vm.prank(alice);
        pool.borrow(address(borrowToken), 5_000 ether); // $5000 borrow
        assertGt(pool.userDebt(alice, address(borrowToken)), 0);
    }

    function test_borrow_exceeds_ltv_reverts() public {
        vm.prank(alice);
        pool.supply(address(collateralToken), 10 ether); // $10,000
        vm.prank(alice);
        vm.expectRevert();
        pool.borrow(address(borrowToken), 9_000 ether); // >75% LTV
    }

    function test_borrow_without_collateral_reverts() public {
        vm.prank(bob);
        vm.expectRevert();
        pool.borrow(address(borrowToken), 1 ether);
    }

    // ── Repay ─────────────────────────────────────────────────────────────

    function test_repay_reduces_debt() public {
        vm.prank(alice);
        pool.supply(address(collateralToken), 10 ether);
        vm.prank(alice);
        pool.borrow(address(borrowToken), 1_000 ether);

        uint256 debtBefore = pool.userDebt(alice, address(borrowToken));
        vm.prank(alice);
        pool.repay(address(borrowToken), 500 ether);
        assertLt(pool.userDebt(alice, address(borrowToken)), debtBefore);
    }

    // ── Withdraw ──────────────────────────────────────────────────────────

    function test_withdraw_collateral_no_debt() public {
        vm.prank(alice);
        pool.supply(address(collateralToken), 10 ether);
        uint256 before = collateralToken.balanceOf(alice);
        vm.prank(alice);
        pool.withdraw(address(collateralToken), 5 ether);
        assertEq(collateralToken.balanceOf(alice), before + 5 ether);
    }

    function test_withdraw_leaves_undercollateralised_reverts() public {
        vm.prank(alice);
        pool.supply(address(collateralToken), 10 ether);
        vm.prank(alice);
        pool.borrow(address(borrowToken), 7_000 ether); // close to LTV limit
        vm.prank(alice);
        vm.expectRevert();
        pool.withdraw(address(collateralToken), 9 ether); // would break LTV
    }

    // ── Pause ─────────────────────────────────────────────────────────────

    function test_pause_blocks_supply() public {
        pool.pause();
        vm.prank(alice);
        vm.expectRevert();
        pool.supply(address(collateralToken), 10 ether);
    }
}
