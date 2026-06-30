# ZRC-721 NFT Standard

**Version**: 1.0  
**Chain**: Zebvix Chain (Chain ID 8989)  
**Status**: Draft  
**Analogous to**: ERC-721 (Ethereum)

---

## Overview

ZRC-721 is the **non-fungible token (NFT) standard for Zebvix Chain**.  
Every ZRC-721 NFT has a unique `tokenId` and can represent:
- Digital art / collectibles
- Game items
- Domain names (ZBX Domains)
- Real-world asset certificates
- Membership passes

---

## Standard Methods

| Method | Description |
|--------|-------------|
| `balanceOf(owner)` | How many NFTs an address owns |
| `ownerOf(tokenId)` | Who owns a specific NFT |
| `transferFrom(from, to, tokenId)` | Transfer ownership |
| `approve(to, tokenId)` | Approve one transfer |
| `setApprovalForAll(op, bool)` | Approve all transfers to operator |
| `tokenURI(tokenId)` | Metadata URI (IPFS recommended) |
| `totalSupply()` | Collection size |

---

## ZRC-721 Extensions

### 1. Batch Mint
```solidity
function batchMint(address to, uint256 quantity)
    external returns (uint256[] memory tokenIds);
```
Mint up to **200 NFTs in one transaction**.

### 2. EIP-2981 Royalties (built-in)
```solidity
function royaltyInfo(uint256 tokenId, uint256 salePrice)
    external view returns (address receiver, uint256 royaltyAmount);
```
Creators get royalties on every secondary sale. Max royalty: **10%** (1000 bps).

---

## Deploying a ZRC-721 Collection

```solidity
contract MyNFT is ZRC721Base {
    address public owner;

    constructor() ZRC721Base("My Collection", "MYC", "ipfs://QmXXX/") {
        owner = msg.sender;
        setDefaultRoyalty(msg.sender, 500); // 5% royalty
    }

    function mint(address to) external {
        require(msg.sender == owner, "not owner");
        _mint(to);
    }

    function batchMint(address to, uint256 qty) external override returns (uint256[] memory) {
        require(msg.sender == owner, "not owner");
        return super.batchMint(to, qty);
    }
}
```

---

## Metadata Standard (IPFS)

Each token's `tokenURI` should point to a JSON file:

```json
{
  "name":        "My NFT #1",
  "description": "First NFT in My Collection",
  "image":       "ipfs://QmImageXXX/1.png",
  "attributes": [
    { "trait_type": "Rarity",  "value": "Legendary" },
    { "trait_type": "Level",   "value": 42 }
  ]
}
```

---

## ABI Selectors

| Function | Selector |
|----------|----------|
| `ownerOf(uint256)` | `0x6352211e` |
| `balanceOf(address)` | `0x70a08231` |
| `transferFrom(address,address,uint256)` | `0x23b872dd` |
| `tokenURI(uint256)` | `0xc87b56dd` |
| `batchMint(address,uint256)` | `0x4a4c560b` |
| `royaltyInfo(uint256,uint256)` | `0x2a55205a` |