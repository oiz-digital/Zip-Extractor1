// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../ZbxOracle.sol";

contract ZbxOracleTest is Test {
    ZbxOracle oracle;

    address owner    = address(this);
    address provider1 = address(0xP1);
    address provider2 = address(0xP2);
    address provider3 = address(0xP3);
    address asset    = address(0xA5537);

    function setUp() public {
        oracle = new ZbxOracle(owner);
        oracle.addProvider(provider1);
        oracle.addProvider(provider2);
        oracle.addProvider(provider3);
    }

    // ── Submit price ──────────────────────────────────────────────────────

    function test_provider_can_submit_price() public {
        vm.prank(provider1);
        oracle.submitPrice(asset, 1_000_000_000); // $10.00 with 8 dec
        // No revert = success
    }

    function test_non_provider_cannot_submit() public {
        vm.prank(address(0xBAD));
        vm.expectRevert();
        oracle.submitPrice(asset, 100);
    }

    // ── Aggregated price ─────────────────────────────────────────────────

    function test_price_aggregates_median_of_three() public {
        vm.prank(provider1); oracle.submitPrice(asset, 900);
        vm.prank(provider2); oracle.submitPrice(asset, 1_000);
        vm.prank(provider3); oracle.submitPrice(asset, 1_100);
        (int256 price,) = oracle.getPrice(asset);
        assertEq(price, 1_000); // median
    }

    function test_price_requires_quorum() public {
        // Only one submission → should revert (not enough providers)
        vm.prank(provider1);
        oracle.submitPrice(asset, 1_000);
        vm.expectRevert();
        oracle.getPrice(asset);
    }

    // ── Staleness ─────────────────────────────────────────────────────────

    function test_stale_price_reverts_on_read() public {
        vm.prank(provider1); oracle.submitPrice(asset, 1_000);
        vm.prank(provider2); oracle.submitPrice(asset, 1_000);
        vm.prank(provider3); oracle.submitPrice(asset, 1_000);

        vm.warp(block.timestamp + 2 hours); // past max staleness
        vm.expectRevert();
        oracle.getPrice(asset);
    }

    // ── Batch submit ──────────────────────────────────────────────────────

    function test_batch_submit() public {
        address[] memory assets = new address[](2);
        assets[0] = address(0xAA);
        assets[1] = address(0xBB);
        int256[] memory prices = new int256[](2);
        prices[0] = 500;
        prices[1] = 750;
        vm.prank(provider1);
        oracle.submitPriceBatch(assets, prices);
        // No revert
    }

    function test_batch_mismatched_lengths_reverts() public {
        address[] memory assets = new address[](2);
        int256[] memory prices  = new int256[](1);
        vm.prank(provider1);
        vm.expectRevert();
        oracle.submitPriceBatch(assets, prices);
    }

    // ── USD value ────────────────────────────────────────────────────────

    function test_get_usd_value() public {
        vm.prank(provider1); oracle.submitPrice(asset, 2_000_000_000); // $20
        vm.prank(provider2); oracle.submitPrice(asset, 2_000_000_000);
        vm.prank(provider3); oracle.submitPrice(asset, 2_000_000_000);
        uint256 usd = oracle.getUSDValue(asset, 5 ether); // 5 tokens × $20 = $100
        assertGt(usd, 0);
    }

    // ── Provider management ──────────────────────────────────────────────

    function test_add_provider() public {
        address newP = address(0xNEW);
        oracle.addProvider(newP);
        assertTrue(oracle.isProvider(newP));
    }

    function test_remove_provider() public {
        oracle.removeProvider(provider1);
        assertFalse(oracle.isProvider(provider1));
    }

    function test_non_owner_cannot_add_provider() public {
        vm.prank(provider1);
        vm.expectRevert();
        oracle.addProvider(address(0xBAD));
    }
}
