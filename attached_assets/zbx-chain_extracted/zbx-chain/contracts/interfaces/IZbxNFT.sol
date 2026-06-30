// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxNFT — Interface for ZbxNFT (ZRC-721) non-fungible token contract.
interface IZbxNFT {
    event Transfer(address indexed from, address indexed to, uint256 indexed tokenId);
    event Approval(address indexed owner, address indexed approved, uint256 indexed tokenId);
    event ApprovalForAll(address indexed owner, address indexed operator, bool approved);
    event Minted(address indexed to, uint256 indexed tokenId, string uri);

    function mint(address to, string calldata uri) external returns (uint256 tokenId);
    function burn(uint256 tokenId) external;
    function balanceOf(address owner) external view returns (uint256);
    function ownerOf(uint256 tokenId) external view returns (address);
    function safeTransferFrom(address from, address to, uint256 tokenId, bytes calldata data) external;
    function safeTransferFrom(address from, address to, uint256 tokenId) external;
    function transferFrom(address from, address to, uint256 tokenId) external;
    function approve(address to, uint256 tokenId) external;
    function setApprovalForAll(address operator, bool approved) external;
    function getApproved(uint256 tokenId) external view returns (address);
    function isApprovedForAll(address owner, address operator) external view returns (bool);
    function tokenURI(uint256 tokenId) external view returns (string memory);
    function totalSupply() external view returns (uint256);
    function supportsInterface(bytes4 interfaceId) external view returns (bool);
}
