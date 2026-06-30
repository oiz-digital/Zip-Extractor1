# ZEP-038: No-Code Smart Contract Factory

| Field       | Value                              |
|-------------|------------------------------------|
| ZEP         | ZEP-038                            |
| Title       | No-Code Smart Contract Factory     |
| Author      | Zebvix Core Team                   |
| Status      | IMPLEMENTED                        |
| Category    | Standard / Infrastructure          |
| Created     | 2026-05-05                         |
| Contracts   | ZbxContractFactory.sol             |

---

## Abstract

ZEP-038 defines a no-code contract factory enabling anyone to deploy standard on-chain primitives (ERC-20 tokens, ERC-721 NFT collections) without writing Solidity. All deployed contracts are recorded in a public registry queryable by creator or globally.

---

## Motivation

Non-technical users and teams without Solidity engineers cannot deploy tokens or NFT collections easily. A factory lowers the barrier by providing one-click deployment of audited, standard contract templates.

---

## Specification

### Supported Contract Types

| Type | Contract | Description |
|------|----------|-------------|
| ERC-20 | `MiniERC20` | Standard fungible token, optional mintability |
| ERC-721 | `MiniNFT` | NFT collection with max supply + EIP-2981 royalties |

### ERC-20 Deployment

```solidity
function deployToken(
    string  name,
    string  symbol,
    uint8   decimals,
    uint256 initialSupply,
    bool    mintable
) external payable returns (address token)
```

Deployed token features:
- Standard `transfer`, `transferFrom`, `approve`
- Optional `mint(to, amount)` — owner-only if `mintable=true`
- `burn(amount)` — any holder
- Initial supply minted to `msg.sender`

### ERC-721 Deployment

```solidity
function deployNFT(
    string  name,
    string  symbol,
    uint256 maxSupply,
    uint96  royaltyBps,
    string  baseUri
) external payable returns (address nft)
```

Deployed collection features:
- `mint(to, amount)` — owner-only, respects `maxSupply` (0 = unlimited)
- `tokenURI(id)` — returns `baseUri + id`
- EIP-2981 royalty info
- EIP-165 interface support

### Registry

Every deployed contract is stored in `registry[]` and `byCreator[address][]`:

```solidity
struct DeployedContract {
    address addr;
    string  contractType;   // "ERC20" | "ERC721"
    address creator;
    uint256 deployedAt;
}
```

**Query functions:**
- `allContracts(offset, limit)` — paginated global list
- `contractsByCreator(address)` — all contracts by a given creator
- `registryLength()` — total deployed contracts

### Anti-Spam Fee

`deployFee = 0.001 ZBX` (configurable by owner). Forwarded to treasury.

---

## Security Considerations

| Risk | Mitigation |
|------|-----------|
| Spam deployments | `deployFee` anti-spam gate |
| Royalty abuse | `royaltyBps <= 1000` (max 10%) enforced |
| Registry poisoning | Registry is append-only; no state is deleted |
| Excess ETH refund | Exact `deployFee` forwarded; excess refunded |

---

## Future Extensions (v2)

- ERC-1155 multi-token factory
- Vesting schedule factory (wraps ZRC20Vesting)
- Multisig factory (wraps ZbxMultisig)
- CREATE2 deterministic addresses

---

## Implementation

- **Contract:** `zbx-chain-extracted/zbx-chain/contracts/ZbxContractFactory.sol`
- **Embedded templates:** `MiniERC20`, `MiniNFT` (defined inline, no external deps)

---

## Status

IMPLEMENTED — Session 46 (2026-05-05). 0 audit findings.
