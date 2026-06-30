// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../ZbxOracleConsumer.sol";

contract MockChainlinkFeed {
    int256 private _answer;
    uint256 private _updatedAt;
    uint8 private _decimals;

    constructor(int256 answer, uint256 updatedAt, uint8 dec) {
        _answer = answer;
        _updatedAt = updatedAt;
        _decimals = dec;
    }

    function latestRoundData() external view returns (
        uint80, int256 answer, uint256, uint256 updatedAt, uint80
    ) {
        return (1, _answer, 0, _updatedAt, 1);
    }

    function decimals() external view returns (uint8) { return _decimals; }
}

contract ZbxOracleConsumerTest is Test {
    ZbxOracleConsumer consumer;
    MockChainlinkFeed zbxFeed;
    MockChainlinkFeed zusdFeed;

    function setUp() public {
        zbxFeed  = new MockChainlinkFeed(50_000_000, block.timestamp, 8); // $0.50
        zusdFeed = new MockChainlinkFeed(100_000_000, block.timestamp, 8); // $1.00

        consumer = new ZbxOracleConsumer(address(zbxFeed), address(zusdFeed));
    }

    // ── ZBX price ─────────────────────────────────────────────────────────

    function test_get_zbx_price_positive() public view {
        (int256 price, uint256 age) = consumer.getZbxPrice();
        assertGt(price, 0);
        assertLe(age, 3600); // fresh
    }

    function test_get_zbx_price_is_50_cents() public view {
        (int256 price,) = consumer.getZbxPrice();
        assertEq(price, 50_000_000);
    }

    // ── ZUSD peg ─────────────────────────────────────────────────────────

    function test_zusd_peg_at_dollar() public view {
        (int256 price, bool isPegged) = consumer.getZusdPeg();
        assertEq(price, 100_000_000);
        assertTrue(isPegged);
    }

    function test_zusd_depeg_detection() public {
        // Set zUSD at $0.90 (depegged)
        MockChainlinkFeed depegFeed = new MockChainlinkFeed(
            90_000_000, block.timestamp, 8
        );
        ZbxOracleConsumer dc = new ZbxOracleConsumer(
            address(zbxFeed), address(depegFeed)
        );
        (, bool isPegged) = dc.getZusdPeg();
        assertFalse(isPegged);
    }

    // ── USD to ZBX conversion ─────────────────────────────────────────────

    function test_usd_to_zbx_conversion() public view {
        // $100 USD → 200 ZBX at $0.50/ZBX
        int256 zbxAmount = consumer.usdToZbx(100_000_000_00); // $100 with 8 dec
        assertGt(zbxAmount, 0);
    }

    // ── Stale price rejection ─────────────────────────────────────────────

    function test_stale_price_reverts() public {
        MockChainlinkFeed staleFeed = new MockChainlinkFeed(
            50_000_000, block.timestamp - 3700, 8 // over 1 hour old
        );
        ZbxOracleConsumer staleConsumer = new ZbxOracleConsumer(
            address(staleFeed), address(zusdFeed)
        );
        vm.expectRevert();
        staleConsumer.getZbxPrice();
    }

    // ── Collateralization check ───────────────────────────────────────────

    function test_is_collateralized() public view {
        // $100 of ZBX at 150% collateral ratio should pass for $50 debt
        bool ok = consumer.isCollateralized(
            200 ether,  // collateral: 200 ZBX at $0.50 = $100
            100 ether,  // debt: 100 ZUSD = $100
            15000       // 150% ratio (bps: 15000 = 150%)
        );
        assertTrue(ok);
    }

    function test_undercollateralized_returns_false() public view {
        bool ok = consumer.isCollateralized(
            10 ether,   // collateral: 10 ZBX at $0.50 = $5
            100 ether,  // debt: 100 ZUSD
            15000
        );
        assertFalse(ok);
    }
}
