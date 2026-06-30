// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import "forge-std/Test.sol";
import "../ZbxNFT.sol";

contract ZbxNFTTest is Test {
    ZbxNFT nft;

    address owner  = address(this);
    address alice  = address(0xA11CE);
    address bob    = address(0xB0B);
    address artist = address(0xAA7157);

    string constant BASE_URI = "ipfs://bafybeig37ioir/";

    function setUp() public {
        nft = new ZbxNFT(
            "ZBX Collection",
            "ZBXNFT",
            BASE_URI,
            10_000,   // maxSupply
            artist,   // royaltyReceiver
            500       // 5% royalty (500 bps)
        );
    }

    // ── Mint ──────────────────────────────────────────────────────────────

    function test_mint_increments_supply() public {
        nft.mint(alice, 1);
        assertEq(nft.totalSupply(), 1);
        assertEq(nft.balanceOf(alice), 1);
    }

    function test_mint_sets_owner() public {
        nft.mint(alice, 1);
        assertEq(nft.ownerOf(1), alice);
    }

    function test_mint_multiple() public {
        nft.mint(alice, 5);
        assertEq(nft.balanceOf(alice), 5);
        assertEq(nft.totalSupply(), 5);
    }

    function test_non_minter_cannot_mint() public {
        vm.prank(alice);
        vm.expectRevert();
        nft.mint(bob, 1);
    }

    function test_max_supply_enforced() public {
        nft.mint(alice, 10_000);
        vm.expectRevert();
        nft.mint(bob, 1);
    }

    // ── Transfer ─────────────────────────────────────────────────────────

    function test_transfer_changes_owner() public {
        nft.mint(alice, 1);
        vm.prank(alice);
        nft.transferFrom(alice, bob, 1);
        assertEq(nft.ownerOf(1), bob);
        assertEq(nft.balanceOf(alice), 0);
        assertEq(nft.balanceOf(bob), 1);
    }

    function test_unauthorized_transfer_reverts() public {
        nft.mint(alice, 1);
        vm.prank(bob);
        vm.expectRevert();
        nft.transferFrom(alice, bob, 1);
    }

    function test_approved_transfer() public {
        nft.mint(alice, 1);
        vm.prank(alice);
        nft.approve(bob, 1);
        vm.prank(bob);
        nft.transferFrom(alice, bob, 1);
        assertEq(nft.ownerOf(1), bob);
    }

    function test_operator_transfer() public {
        nft.mint(alice, 1);
        vm.prank(alice);
        nft.setApprovalForAll(bob, true);
        assertTrue(nft.isApprovedForAll(alice, bob));
        vm.prank(bob);
        nft.transferFrom(alice, bob, 1);
        assertEq(nft.ownerOf(1), bob);
    }

    // ── Soulbound ────────────────────────────────────────────────────────

    function test_soulbound_token_cannot_be_transferred() public {
        nft.mintSoulbound(alice, 1);
        vm.prank(alice);
        vm.expectRevert();
        nft.transferFrom(alice, bob, 1);
    }

    // ── Royalty (ERC-2981) ────────────────────────────────────────────────

    function test_royalty_info() public view {
        (address receiver, uint256 amount) = nft.royaltyInfo(1, 10_000);
        assertEq(receiver, artist);
        assertEq(amount, 500); // 5% of 10_000
    }

    // ── ERC-165 ──────────────────────────────────────────────────────────

    function test_supports_erc721_interface() public view {
        assertTrue(nft.supportsInterface(0x80ac58cd)); // ERC-721
    }

    function test_supports_erc2981_interface() public view {
        assertTrue(nft.supportsInterface(0x2a55205a)); // ERC-2981
    }

    // ── Token URI ─────────────────────────────────────────────────────────

    function test_token_uri_has_base() public {
        nft.mint(alice, 1);
        string memory uri = nft.tokenURI(1);
        assertTrue(bytes(uri).length > 0);
    }
}
