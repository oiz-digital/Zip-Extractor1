// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxNftMarketplace — Interface for ZbxNftMarketplace NFT trading contract.
interface IZbxNftMarketplace {
    event Listed(address indexed seller, address indexed nft, uint256 indexed tokenId, uint256 price, uint256 nonce, uint256 expiry);
    event Sold(address indexed buyer, address indexed seller, address indexed nft, uint256 tokenId, uint256 price);
    event ListingCancelled(address indexed seller, uint256 nonce);
    event AllListingsCancelled(address indexed seller, uint256 throughNonce);

    error FeeTooHigh();
    error ZeroTreasury();
    error ListingExpired();
    error ListingCancelledErr();

    function buy(address nft, uint256 tokenId, address seller, uint256 price, uint256 nonce, uint256 expiry, bytes calldata sig) external payable;
    function cancelListing(uint256 nonce) external;
    function cancelAllListings(uint256 throughNonce) external;
    function setFee(uint256 feeBps) external;
    function setTreasury(address treasury) external;
    function feeBps() external view returns (uint256);
    function treasury() external view returns (address);
    function minNonce(address seller) external view returns (uint256);
}
