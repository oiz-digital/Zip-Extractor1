# ZEP-029: Token Creator Platform

| Field     | Value |
|-----------|-------|
| ZEP       | 029 |
| Title     | Token Creator Platform (No-Code) |
| Author    | Zebvix Core Team |
| Status    | Accepted |
| Category  | Developer Experience / DeFi |
| Created   | 2026-06-28 |
| Requires  | ZEP-006 (ZRC20 Advanced) |

---

## Abstract

The Zebvix Token Creator Platform enables non-technical users to deploy ZRC-20, ZRC-721, ZRC-1155, and DAO contracts with a single transaction — no Solidity knowledge required. A web-based wizard collects token parameters and invokes the factory contracts.

---

## Motivation

Most L1 ecosystems gate token creation behind technical knowledge (Solidity, Hardhat, deployment scripts). This excludes the vast majority of potential creators — artists, game developers, communities, DAOs — who have ideas but not engineering resources.

Zebvix Token Creator solves this by:
1. Abstracting deployment complexity into factory contracts
2. Providing a no-code web wizard at `https://create.zebvix.com`
3. Automatically registering deployed tokens in the explorer

---

## Token Types

### ZRC-20 (Fungible Token)

Features configurable at deploy time:
- Name, symbol, decimals
- Initial supply + max supply cap
- Mintable / burnable / pausable flags
- Anti-bot launch protection (max transfer limit for N blocks)
- Deploy fee: 10 ZBX

Factory: `ZRC20Creator.sol` (deployed this commit)

### ZRC-721 (NFT Collection)

Features:
- Collection name, symbol
- Max supply, mint price
- IPFS base URI + reveal mechanism (hidden URI before launch)
- Allowlist / whitelist phases
- ERC-2981 royalties (configurable %)
- Owner airdrop mint
- Deploy fee: 25 ZBX

Factory: `ZRC721Creator.sol` (deployed this commit)

### ZRC-1155 (Multi-Token)

Features:
- Multiple token types in one contract (fungible + NFTs)
- Per-token-type supply caps and mint prices
- Batch transfers
- ERC-2981 royalties
- Deploy fee: 25 ZBX

Factory: `ZRC1155Creator.sol` (deployed this commit)

### DAO (Governance + Treasury)

Features:
- Governance token (ZRC-20) deployed alongside DAO
- On-chain proposal → vote → timelock → execute cycle
- Configurable: voting delay, voting period, quorum %, proposal threshold
- Treasury (ETH/ZBX + tokens) managed by governance
- Deploy fee: 50 ZBX

Factory: `DAOCreator.sol` (deployed this commit)

---

## No-Code Wizard

URL: `https://create.zebvix.com`

Steps:
1. Connect wallet
2. Choose token type (ZRC-20 / ZRC-721 / ZRC-1155 / DAO)
3. Fill in parameters (form with validation)
4. Preview contract summary
5. Sign transaction (MetaMask / WalletConnect)
6. View deployed contract on explorer

---

## Factory Registry

Each factory emits an event on deployment:
- `ZRC20Creator`: `TokenDeployed(address, creator, name, symbol, supply)`
- `ZRC721Creator`: `CollectionDeployed(address, creator, name, symbol, maxSupply)`
- `ZRC1155Creator`: `ContractDeployed(address, creator, name)`
- `DAOCreator`: `DAODeployed(address, govToken, creator, name)`

The block explorer indexes these events to auto-populate the token registry.

---

## Fee Structure

| Token Type | Deploy Fee |
|------------|-----------|
| ZRC-20 | 10 ZBX |
| ZRC-721 | 25 ZBX |
| ZRC-1155 | 25 ZBX |
| DAO | 50 ZBX |

Fees collected by factory owner → Zebvix Development Fund.

---

## Security

- Factory contracts are non-upgradeable.
- Anti-reentrancy on fee collection.
- Deployed token contracts owned by the creator — not the factory.
- Fee withdrawal uses a pull pattern (no automatic sends).

---

## Reference Implementation

- `contracts/ZRC20Creator.sol` ✅
- `contracts/ZRC721Creator.sol` ✅
- `contracts/ZRC1155Creator.sol` ✅
- `contracts/DAOCreator.sol` ✅

---

## Backwards Compatibility

New contracts; no existing interfaces modified.
