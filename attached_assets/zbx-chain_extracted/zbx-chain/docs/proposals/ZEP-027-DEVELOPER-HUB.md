# ZEP-027: Zebvix Developer Hub

| Field     | Value |
|-----------|-------|
| ZEP       | 027 |
| Title     | Developer Hub |
| Author    | Zebvix Core Team |
| Status    | Accepted |
| Category  | Developer Experience |
| Created   | 2026-06-28 |
| Requires  | ZEP-001 (PayID), ZEP-004 (ZVM) |

---

## Abstract

This ZEP defines the Zebvix Developer Hub — a production-ready developer portal that provides API keys, a testnet faucet, RPC dashboard, contract verification, SDK downloads, documentation, analytics, and example projects in a single unified interface.

---

## Motivation

Zebvix Chain has all the core infrastructure required for testnet launch (consensus, EVM, ZVM, bridge, staking). The critical missing piece for developer adoption is a professional developer experience surface. Without:

- Easy API key access
- Testnet faucet
- Contract verification
- Interactive documentation

…developers will struggle to build on Zebvix Chain, reducing ecosystem growth.

---

## Specification

### 1. Developer Portal

URL: `https://dev.zebvix.com`

Pages:

| Route | Description |
|-------|-------------|
| `/` | Dashboard overview |
| `/api-keys` | Generate and manage RPC API keys |
| `/faucet` | Testnet ZBX faucet (1 ZBX/day/address) |
| `/rpc` | RPC endpoint dashboard with latency and rate limit info |
| `/verify` | Smart contract verification (upload source + ABI) |
| `/sdk` | SDK download page (Rust, JS/TS, Go, Python, Flutter) |
| `/docs` | Interactive API documentation (OpenAPI + GraphQL) |
| `/analytics` | Chain analytics: transactions/day, active users, TVL |
| `/examples` | Curated example projects with one-click fork |
| `/tutorials` | Step-by-step guides (Deploy contract, Run validator, Bridge) |
| `/node-setup` | Validator and full-node setup guides |

---

### 2. API Key System

API keys gate access to production RPC endpoints. Design:

```
Key format: zbx_live_<32-byte-hex>  (mainnet)
            zbx_test_<32-byte-hex>  (testnet)
```

Rate limits:

| Tier | Requests/day | Batch size |
|------|-------------|------------|
| Free | 50,000 | 10 |
| Developer | 500,000 | 100 |
| Pro | 10,000,000 | 1,000 |

Keys are validated in the nginx upstream or a lightweight Axum middleware that reads from Redis.

---

### 3. Testnet Faucet

- Dispenses 1 ZBX per address per 24 hours.
- Verification: EVM address + optional Twitter/GitHub OAuth.
- Anti-abuse: rate limit by IP, address, and social account.
- Backed by a dedicated faucet address seeded in `testnet-genesis.json` (already allocated 10M ZBX — see F7 in AUDIT_2026-06-28.md).

Faucet API:

```http
POST /api/v1/faucet
Content-Type: application/json

{ "address": "0x...", "chain_id": 8990 }

→ 200: { "tx_hash": "0x...", "amount": "1000000000000000000" }
→ 429: { "error": "Rate limited", "retry_after": 86400 }
```

---

### 4. Contract Verification

- Accepts Solidity source + compiler settings.
- Compiles locally (solc 0.8.24) and compares bytecode with on-chain deployment.
- Stores verified ABI + source in RocksDB (keyed by contract address).
- Explorer automatically links verified contracts.

---

### 5. RPC Dashboard

Displays per-key metrics:
- Requests / hour (bar chart)
- p50 / p95 / p99 latency
- Error rate breakdown (by RPC method)
- Current rate limit usage

---

### 6. Analytics

Public chain analytics:
- Transactions per day (7d, 30d)
- Active addresses (unique senders)
- ZBX staked (% of supply)
- Bridge volume (by chain)
- Gas price history

---

## Implementation

### Rust backend (`node/src/dev_hub.rs`)

```rust
pub struct DevHubConfig {
    pub bind:          SocketAddr,   // default: 0.0.0.0:9000
    pub faucet_key:    [u8; 32],     // faucet wallet private key (env: ZBX_FAUCET_KEY)
    pub faucet_amount: u128,         // wei per drip (default: 1 ZBX)
    pub drip_cooldown: Duration,     // default: 24h
    pub redis_url:     String,       // for rate limiting state
}
```

### Frontend (`sdk/developer-hub/`)

- React + Vite + Tailwind
- Dark/light mode
- Wallet connect (MetaMask / WalletConnect)

---

## Security

- API keys stored as `HMAC-SHA256(secret, key_id)` in PostgreSQL.
- Faucet private key loaded from environment only — never serialized.
- Rate limit state in Redis with TTL-based sliding window.

---

## Backwards Compatibility

This ZEP adds new services; no existing APIs are modified.

---

## Reference Implementation

- `node/src/dev_hub.rs` — faucet + API key service
- `crates/zbx-rest/` — REST API (ZEP-027 routes)
- `contracts/ZbxFaucet.sol` — on-chain faucet record (optional)
