// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZRC721 — ZRC-721 NFT Standard Interface (ZBX Chain).
/// @notice Zebvix Chain's non-fungible token standard, fully compatible
///         with ERC-721 so all existing NFT tooling works out-of-the-box.
///
/// @dev Extensions bundled into the standard:
///        1. ERC-721Metadata (name, symbol, tokenURI)
///        2. ERC-721Enumerable (totalSupply, tokenByIndex, tokenOfOwnerByIndex)
///        3. On-chain royalties (EIP-2981)
///        4. Batch mint

interface IZRC721 {

    // ─── Events ────────────────────────────────────────────────────────────

    event Transfer(address indexed from, address indexed to,      uint256 indexed tokenId);
    event Approval(address indexed owner, address indexed approved, uint256 indexed tokenId);
    event ApprovalForAll(address indexed owner, address indexed operator, bool approved);

    // ─── ERC-721 Core ──────────────────────────────────────────────────────

    function balanceOf(address owner) external view returns (uint256);
    function ownerOf(uint256 tokenId) external view returns (address);
    function safeTransferFrom(address from, address to, uint256 tokenId, bytes calldata data) external;
    function safeTransferFrom(address from, address to, uint256 tokenId) external;
    function transferFrom(address from, address to, uint256 tokenId) external;
    function approve(address to, uint256 tokenId) external;
    function setApprovalForAll(address operator, bool approved) external;
    function getApproved(uint256 tokenId) external view returns (address);
    function isApprovedForAll(address owner, address operator) external view returns (bool);

    // ─── Metadata ─────────────────────────────────────────────────────────

    function name()                           external view returns (string memory);
    function symbol()                         external view returns (string memory);
    function tokenURI(uint256 tokenId)        external view returns (string memory);

    // ─── Enumerable ───────────────────────────────────────────────────────

    function totalSupply()                                  external view returns (uint256);
    function tokenByIndex(uint256 index)                    external view returns (uint256);
    function tokenOfOwnerByIndex(address owner, uint256 index) external view returns (uint256);

    // ─── ZRC-721 Extension: Batch Mint ───────────────────────────────────

    function batchMint(address to, uint256 quantity) external returns (uint256[] memory tokenIds);

    // ─── ZRC-721 Extension: EIP-2981 Royalties ───────────────────────────

    function royaltyInfo(uint256 tokenId, uint256 salePrice)
        external view returns (address receiver, uint256 royaltyAmount);

    function setDefaultRoyalty(address receiver, uint96 feeBasisPoints) external;
}