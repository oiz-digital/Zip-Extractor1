# ZEP-028: Zebvix App Store Registry

| Field     | Value |
|-----------|-------|
| ZEP       | 028 |
| Title     | App Store Registry |
| Author    | Zebvix Core Team |
| Status    | Accepted |
| Category  | Ecosystem |
| Created   | 2026-06-28 |
| Requires  | ZEP-004 (ZVM), ZEP-017 (Account Abstraction) |

---

## Abstract

This ZEP defines the Zebvix App Store — an on-chain decentralised application registry. Developers publish apps with versioned bundles, metadata, and category tags. Users discover, rate, and install apps directly from the chain. The app store is the primary growth surface for the Zebvix ecosystem.

---

## Motivation

A decentralised app store is a key differentiator for Zebvix Chain over generic L1s:

1. **Developer discovery** — apps are automatically indexed and discoverable by the block explorer.
2. **Trust** — the registry is on-chain and immutable; no central authority can remove an app.
3. **Monetisation** — apps can charge install fees or earn from app store referrals.
4. **Governance** — the community can vote to feature or flag malicious apps.

---

## Specification

### App Categories

- `wallet` — key management and signing tools
- `defi` — DEX, lending, yield, perps
- `nft` — NFT minting, marketplace, gallery
- `ai_tools` — AI-powered on/off-chain tools
- `games` — ZVM-based or on-chain games
- `utilities` — explorers, bridges, analytics

### App Metadata

```rust
struct AppMetadata {
    slug:                String,      // kebab-case, globally unique
    name:                String,      // human-readable name
    tagline:             String,      // ≤256 chars
    description:         String,      // ≤4096 chars
    category:            AppCategory,
    publisher:           Address,
    icon_url:            String,      // IPFS CID or HTTPS
    bundle_url:          String,      // IPFS CID or HTTPS
    current_version:     String,      // semver
    status:              AppStatus,   // Active | Removed | Suspended
    tags:                Vec<String>, // ≤10
    published_at_block:  u64,
    updated_at_block:    u64,
}
```

### Versioning

Apps publish semver-versioned releases. Each version has:
- Bundle URL (IPFS CID preferred)
- SHA-256 checksum of bundle
- Release notes
- Minimum chain version required

### Rating System

- 1–5 star ratings per user per app
- Aggregate: count, total_stars, distribution[5]
- One rating per wallet address (updated in place)

### Install Tracking

- On-chain install counter (RocksDB: `installs/{slug}`)
- Per-user install records

---

## On-Chain Storage Layout

```
apps/{slug}                  → AppMetadata (CBOR)
apps/{slug}/versions/{v}     → VersionRecord (CBOR)
categories/{cat}/{slug}      → "" (index entry)
ratings/{slug}/summary       → RatingSummary (CBOR)
ratings/{slug}/{addr}        → RatingRecord (CBOR)
installs/{slug}              → u64 (little-endian count)
installs/{slug}/users/{addr} → InstallRecord (CBOR)
```

---

## App Store ZVM Opcodes

| Opcode | Hex | Description |
|--------|-----|-------------|
| `APPSTORE_PUBLISH` | `0xB0` | Publish or update an app |
| `APPSTORE_INSTALL` | `0xB1` | Record an install |
| `APPSTORE_RATE`    | `0xB2` | Submit a rating |
| `APPSTORE_QUERY`   | `0xB3` | Read app metadata |

---

## Governance Integration

Malicious or fraudulent apps can be suspended via ZBX governance vote:
- Proposal type: `AppSuspension { slug, reason }`
- Quorum: 4% of total staked ZBX
- Timelock: 48 hours

Suspended apps are flagged in search results and the install button is disabled.

---

## Fee Model

| Action | Fee |
|--------|-----|
| Publish app | 100 ZBX (one-time) |
| Publish version | 10 ZBX |
| Rate app | 0 ZBX |
| Install | 0 ZBX (free tier) |

Fees flow to the Zebvix Development Fund treasury.

---

## Reference Implementation

- `crates/zbx-appstore/` — Rust registry crate (this commit)
- `contracts/ZbxAppStore.sol` — on-chain governance hooks
- Explorer integration: auto-index apps from `AppDeployed` events

---

## Backwards Compatibility

New feature; no existing APIs modified.
