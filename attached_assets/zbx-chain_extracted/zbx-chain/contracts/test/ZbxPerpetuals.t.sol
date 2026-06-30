// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../ZbxPerpetuals.sol";

contract MockPerpOracle {
    mapping(address => uint256) private _p;
    function setPrice(address a, uint256 p) external { _p[a] = p; }
    function latestAnswer() external pure returns (int256) { return 1_000 * 1e8; }
    function decimals() external pure returns (uint8) { return 8; }
    function latestRoundData() external view returns (uint80, int256, uint256, uint256, uint80) {
        return (1, 1_000 * 1e8, 0, block.timestamp, 1);
    }
}

contract MockPerpMargin {
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

contract ZbxPerpetualsTest is Test {
    ZbxPerpetuals perps;
    MockPerpOracle oracle;
    MockPerpMargin margin;

    address admin = address(this);
    address alice = address(0xA11CE);
    address bob   = address(0xB0B);
    address underlying = address(0xBTC);
    bytes32 marketId;

    function setUp() public {
        oracle = new MockPerpOracle();
        margin = new MockPerpMargin();
        perps  = new ZbxPerpetuals(admin, address(oracle), address(margin));

        // Add ZBX/USDT perp market
        marketId = perps.addMarket(
            "ZBX-PERP", underlying,
            10,    // 10x max leverage
            1000,  // 10% maintenance margin (1000 bps)
            500    // 0.5% funding rate cap (500 bps)
        );

        margin.mint(alice, 100_000 ether);
        margin.mint(bob,   100_000 ether);
        vm.prank(alice); margin.approve(address(perps), type(uint256).max);
        vm.prank(bob);   margin.approve(address(perps), type(uint256).max);
    }

    // ── Deposit cross-margin ──────────────────────────────────────────────

    function test_deposit_cross_margin() public {
        vm.prank(alice);
        perps.depositCross(5_000 ether);
        assertGt(perps.crossMargin(alice), 0);
    }

    function test_deposit_zero_reverts() public {
        vm.prank(alice);
        vm.expectRevert();
        perps.depositCross(0);
    }

    // ── Open position ─────────────────────────────────────────────────────

    function test_open_long_position() public {
        vm.prank(alice);
        perps.depositCross(10_000 ether);
        vm.prank(alice);
        uint256 posId = perps.openPosition(
            marketId, true, 1 ether, 5, // 1 BTC long, 5x leverage
            1_100 * 1e18 // stop-loss at $1100
        );
        assertGt(posId, 0);
    }

    function test_open_short_position() public {
        vm.prank(alice);
        perps.depositCross(10_000 ether);
        vm.prank(alice);
        uint256 posId = perps.openPosition(
            marketId, false, 1 ether, 5, 900 * 1e18
        );
        assertGt(posId, 0);
    }

    function test_open_position_insufficient_margin_reverts() public {
        vm.prank(alice);
        perps.depositCross(10 ether); // way too low
        vm.prank(alice);
        vm.expectRevert();
        perps.openPosition(marketId, true, 100 ether, 10, 900 * 1e18);
    }

    function test_leverage_exceeds_max_reverts() public {
        vm.prank(alice);
        perps.depositCross(100_000 ether);
        vm.prank(alice);
        vm.expectRevert();
        perps.openPosition(marketId, true, 1 ether, 100, 0); // 100x > 10x max
    }

    // ── Close position ────────────────────────────────────────────────────

    function test_close_position() public {
        vm.prank(alice);
        perps.depositCross(10_000 ether);
        vm.prank(alice);
        uint256 posId = perps.openPosition(marketId, true, 1 ether, 5, 0);
        vm.prank(alice);
        perps.closePosition(posId);
        // Should refund margin (no revert)
    }

    // ── Partial close ─────────────────────────────────────────────────────

    function test_partial_close() public {
        vm.prank(alice);
        perps.depositCross(10_000 ether);
        vm.prank(alice);
        uint256 posId = perps.openPosition(marketId, true, 1 ether, 5, 0);
        vm.prank(alice);
        perps.partialClose(posId, 5000); // close 50%
    }

    // ── Market configuration ──────────────────────────────────────────────

    function test_update_market() public {
        perps.updateMarket(marketId, 20, 800, 300);
        // No revert
    }

    function test_withdraw_cross_margin() public {
        vm.prank(alice);
        perps.depositCross(5_000 ether);
        uint256 before = margin.balanceOf(alice);
        vm.prank(alice);
        perps.withdrawCross(2_000 ether);
        assertEq(margin.balanceOf(alice), before + 2_000 ether);
    }
}
