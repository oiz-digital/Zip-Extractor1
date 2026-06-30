// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../TokenRegistry.sol";

contract TokenRegistryTest is Test {
    TokenRegistry registry;
    address owner  = address(this);
    address alice  = address(0xA11CE);
    address token1 = address(0xT0K1);
    address token2 = address(0xT0K2);

    function setUp() public {
        registry = new TokenRegistry(owner);
    }

    // ── Register ──────────────────────────────────────────────────────────

    function test_register_token() public {
        registry.register(
            token1, "Test Token", "TTK", 18,
            TokenRegistry.TokenCategory.ZRC20,
            "ipfs://logo", "https://test.io"
        );
        assertTrue(registry.isRegistered(token1));
    }

    function test_register_zero_address_reverts() public {
        vm.expectRevert();
        registry.register(
            address(0), "Test", "TTK", 18,
            TokenRegistry.TokenCategory.ZRC20,
            "", ""
        );
    }

    function test_register_duplicate_reverts() public {
        registry.register(token1, "T", "T", 18, TokenRegistry.TokenCategory.ZRC20, "", "");
        vm.expectRevert();
        registry.register(token1, "T", "T", 18, TokenRegistry.TokenCategory.ZRC20, "", "");
    }

    function test_total_tokens_increments() public {
        registry.register(token1, "T1", "T1", 18, TokenRegistry.TokenCategory.ZRC20, "", "");
        registry.register(token2, "T2", "T2", 18, TokenRegistry.TokenCategory.Bridged, "", "");
        assertEq(registry.totalTokens(), 2);
    }

    // ── Verified ─────────────────────────────────────────────────────────

    function test_set_verified() public {
        registry.register(token1, "T", "T", 18, TokenRegistry.TokenCategory.ZRC20, "", "");
        registry.setVerified(token1, true);
        assertTrue(registry.isVerified(token1));
    }

    function test_non_owner_cannot_verify() public {
        registry.register(token1, "T", "T", 18, TokenRegistry.TokenCategory.ZRC20, "", "");
        vm.prank(alice);
        vm.expectRevert();
        registry.setVerified(token1, true);
    }

    // ── Category listing ─────────────────────────────────────────────────

    function test_tokens_in_category() public {
        registry.register(token1, "T1", "T1", 18, TokenRegistry.TokenCategory.ZRC20, "", "");
        registry.register(token2, "T2", "T2", 18, TokenRegistry.TokenCategory.ZRC20, "", "");
        address[] memory listed = registry.tokensInCategory(TokenRegistry.TokenCategory.ZRC20);
        assertEq(listed.length, 2);
    }

    // ── Batch get ─────────────────────────────────────────────────────────

    function test_get_batch() public {
        registry.register(token1, "T1", "T1", 18, TokenRegistry.TokenCategory.ZRC20, "", "");
        registry.register(token2, "T2", "T2", 18, TokenRegistry.TokenCategory.Bridged, "", "");
        address[] memory toks = new address[](2);
        toks[0] = token1;
        toks[1] = token2;
        TokenRegistry.TokenMeta[] memory meta = registry.getBatch(toks);
        assertEq(meta.length, 2);
        assertEq(meta[0].symbol, "T1");
        assertEq(meta[1].symbol, "T2");
    }

    // ── Pagination ────────────────────────────────────────────────────────

    function test_get_tokens_paginated() public {
        for (uint256 i; i < 5; i++) {
            registry.register(
                address(uint160(i + 1)), string(abi.encodePacked("T", i)),
                string(abi.encodePacked("T", i)), 18,
                TokenRegistry.TokenCategory.ZRC20, "", ""
            );
        }
        (address[] memory page,) = registry.getTokens(0, 3);
        assertEq(page.length, 3);
    }

    // ── Ownership ─────────────────────────────────────────────────────────

    function test_transfer_ownership() public {
        registry.transferOwnership(alice);
        vm.prank(alice);
        registry.register(token1, "T", "T", 18, TokenRegistry.TokenCategory.ZRC20, "", "");
        assertTrue(registry.isRegistered(token1));
    }
}
