# ZEP-037: ZBX Name Service (ZNS)

| Field       | Value                                   |
|-------------|-----------------------------------------|
| ZEP         | ZEP-037                                 |
| Title       | ZBX Name Service — Human-Readable Names |
| Author      | Zebvix Core Team                        |
| Status      | IMPLEMENTED                             |
| Category    | Standard / Infrastructure               |
| Created     | 2026-05-05                              |
| Contracts   | ZbxNameService.sol                      |

---

## Abstract

ZEP-037 defines the ZBX Name Service (ZNS), an ENS-compatible human-readable naming system for ZBX Chain. Users register `yourname.zbx` and map it to any wallet address, smart contract, or metadata record. Ownership is tracked as an ERC-721 NFT, enabling names to be traded on any NFT marketplace.

---

## Motivation

ZBX wallet addresses are 42-character hex strings — difficult to share verbally or remember. ZNS maps readable names to addresses, reducing errors and improving UX. Combined with reverse lookup, wallets can display `alice.zbx` instead of `0x1234…abcd`.

---

## Specification

### Registration

```solidity
function register(string calldata name, address addr)
    external payable returns (uint256 tokenId)
```

- Costs `annualFee` (default: 1 ZBX) per year
- Name is normalised to lowercase
- Validated: alphanumeric + hyphens, min 3 chars, no leading/trailing hyphens
- Returns ERC-721 `tokenId` representing ownership
- Excess `msg.value` refunded

### Name Lifecycle

```
Registered → Expired (after 1 year) → Grace Period (30 days) → Available
                     ↑ Renew (owner only, resets +1 year)
```

### Resolution

```solidity
resolve(string name) → address          // forward lookup
reverseLookup(address) → string         // reverse lookup
```

### Records (Metadata)

Arbitrary key-value pairs stored on-chain:

| Key | Example Value |
|-----|--------------|
| `avatar` | `ipfs://Qm...` |
| `email` | `alice@zebvix.io` |
| `url` | `https://alice.xyz` |
| `description` | `ZBX validator operator` |
| `com.twitter` | `@aliceonchain` |

### Subdomains

Parent name owner can issue `sub.parent.zbx`:

```solidity
issueSubdomain(parent, sub, to)
```

Subdomains inherit parent's expiry and are issued as separate ERC-721 tokens.

### ERC-721 Compatibility

- `ownerOf(tokenId)` → name owner
- `transferFrom(from, to, tokenId)` → transfer name
- `approve` / `setApprovalForAll` → marketplace compatibility
- `tokenURI(tokenId)` → on-chain generated (can be IPFS metadata)

---

## Name Validation Rules

| Rule | Detail |
|------|--------|
| Minimum length | 3 characters |
| Allowed characters | `a-z`, `0-9`, `-` (hyphen) |
| Case | Always normalised to lowercase |
| No leading/trailing hyphens | Enforced |
| Unicode | Not supported in v1 (ASCII only) |

---

## Fees

| Action | Cost |
|--------|------|
| Register | `annualFee` (default 1 ZBX) |
| Renew | `annualFee` per year added |
| Transfer | Gas only (no protocol fee) |
| Set record | Gas only |

Revenue goes to contract admin (Zebvix DAO treasury).

---

## Security Considerations

| Risk | Mitigation |
|------|-----------|
| Homoglyph attacks (`A` vs `a`) | Lowercase normalisation in `_normalize()` |
| Grace period race condition | `block.timestamp < expiry + GRACE_PERIOD` enforced |
| Subdomain outliving parent | Subdomain inherits parent expiry — auto-expires |
| Reentrancy on register | CEI: state written before ZBX refund sent |

---

## Implementation

- **Contract:** `zbx-chain-extracted/zbx-chain/contracts/ZbxNameService.sol`
- **Key functions:** `register`, `renew`, `resolve`, `reverseLookup`, `setAddress`, `setPrimaryName`, `setRecord`, `issueSubdomain`, `transferFrom`

---

## Status

IMPLEMENTED — Session 46 (2026-05-05). 0 audit findings.
