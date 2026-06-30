// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../ZbxNameService.sol";

contract ZbxNameServiceTest is Test {
    ZbxNameService zns;

    address owner = address(this);
    address alice = address(0xA11CE);
    address bob   = address(0xB0B);

    uint256 constant YEAR_FEE = 0.1 ether; // typical registration fee per year

    function setUp() public {
        zns = new ZbxNameService();
        vm.deal(alice, 10 ether);
        vm.deal(bob, 10 ether);
    }

    function _fee(string memory name, uint256 years) internal view returns (uint256) {
        return zns.registrationFee(name, years);
    }

    // ── Register ──────────────────────────────────────────────────────────

    function test_register_name() public {
        uint256 fee = _fee("alice", 1);
        vm.prank(alice);
        zns.register{value: fee}("alice", 1, alice);
        assertEq(zns.resolve("alice"), alice);
    }

    function test_register_sets_nft_owner() public {
        uint256 fee = _fee("alice", 1);
        vm.prank(alice);
        zns.register{value: fee}("alice", 1, alice);
        uint256 tokenId = zns.nameToTokenId("alice");
        assertEq(zns.ownerOf(tokenId), alice);
    }

    function test_register_taken_name_reverts() public {
        uint256 fee = _fee("alice", 1);
        vm.prank(alice);
        zns.register{value: fee}("alice", 1, alice);
        vm.prank(bob);
        vm.expectRevert();
        zns.register{value: fee}("alice", 1, bob);
    }

    function test_register_insufficient_fee_reverts() public {
        vm.prank(alice);
        vm.expectRevert();
        zns.register{value: 1 wei}("alice", 1, alice);
    }

    function test_register_invalid_name_reverts() public {
        uint256 fee = _fee("ab", 1);
        vm.prank(alice);
        vm.expectRevert();
        zns.register{value: fee}("ab", 1, alice);
    }

    // ── Resolve ───────────────────────────────────────────────────────────

    function test_resolve_returns_correct_address() public {
        uint256 fee = _fee("myname", 1);
        vm.prank(alice);
        zns.register{value: fee}("myname", 1, alice);
        assertEq(zns.resolve("myname"), alice);
    }

    function test_resolve_unregistered_returns_zero() public view {
        assertEq(zns.resolve("nobody"), address(0));
    }

    // ── Set address ───────────────────────────────────────────────────────

    function test_owner_can_set_address() public {
        uint256 fee = _fee("alice", 1);
        vm.prank(alice);
        zns.register{value: fee}("alice", 1, alice);
        vm.prank(alice);
        zns.setAddress("alice", bob);
        assertEq(zns.resolve("alice"), bob);
    }

    function test_non_owner_cannot_set_address() public {
        uint256 fee = _fee("alice", 1);
        vm.prank(alice);
        zns.register{value: fee}("alice", 1, alice);
        vm.prank(bob);
        vm.expectRevert();
        zns.setAddress("alice", bob);
    }

    // ── Reverse lookup ────────────────────────────────────────────────────

    function test_set_primary_name() public {
        uint256 fee = _fee("alice", 1);
        vm.prank(alice);
        zns.register{value: fee}("alice", 1, alice);
        vm.prank(alice);
        zns.setPrimaryName("alice");
        assertEq(zns.primaryName(alice), "alice.zbx");
    }

    // ── Renewal ───────────────────────────────────────────────────────────

    function test_renewal_extends_expiry() public {
        uint256 fee = _fee("alice", 1);
        vm.prank(alice);
        zns.register{value: fee}("alice", 1, alice);
        uint256 expiry1 = zns.expiry("alice");

        uint256 renewFee = _fee("alice", 1);
        vm.prank(alice);
        zns.renew{value: renewFee}("alice", 1);
        uint256 expiry2 = zns.expiry("alice");
        assertGt(expiry2, expiry1);
    }

    // ── Subdomain ─────────────────────────────────────────────────────────

    function test_issue_subdomain() public {
        uint256 fee = _fee("alice", 1);
        vm.prank(alice);
        zns.register{value: fee}("alice", 1, alice);
        vm.prank(alice);
        zns.issueSubdomain("alice", "shop", bob);
        assertEq(zns.resolve("shop.alice"), bob);
    }

    function test_non_parent_owner_cannot_issue_subdomain() public {
        uint256 fee = _fee("alice", 1);
        vm.prank(alice);
        zns.register{value: fee}("alice", 1, alice);
        vm.prank(bob);
        vm.expectRevert();
        zns.issueSubdomain("alice", "shop", bob);
    }

    // ── Transfer ──────────────────────────────────────────────────────────

    function test_transfer_name_nft() public {
        uint256 fee = _fee("alice", 1);
        vm.prank(alice);
        zns.register{value: fee}("alice", 1, alice);
        uint256 tokenId = zns.nameToTokenId("alice");
        vm.prank(alice);
        zns.transferFrom(alice, bob, tokenId);
        assertEq(zns.ownerOf(tokenId), bob);
    }
}
