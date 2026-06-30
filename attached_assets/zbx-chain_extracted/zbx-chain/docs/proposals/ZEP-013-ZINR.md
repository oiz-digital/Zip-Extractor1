# ZEP-013 — ZINR: Indian Rupee Stablecoin on ZBX Chain (WITHDRAWN)

> **⚠ WITHDRAWN (Session 31 — 2026-05-05)**: ZINR has been removed from ZBX Chain.
> Only ZBX and ZUSD remain as native tokens. This document is preserved as historical record only.

| Field | Value |
|---|---|
| **ZEP** | 013 |
| **Title** | ZINR — Indian Rupee-pegged stablecoin |
| **Author** | Zebvix Foundation |
| **Status** | WITHDRAWN |
| **Category** | Standard |
| **Created** | 2026-05-03 |
| **Activation block** | 1 (genesis-deployed) |

---

## Abstract

ZINR is a native Indian Rupee-pegged stablecoin on ZBX Chain.
1 ZINR = 1 Indian Rupee (INR).

ZINR enables instant, low-cost settlements in INR value without converting to USD
(ZUSD). It is designed for India's $120B/year remittance market, domestic P2P
payments, and DeFi protocols denominated in INR.

---

## Motivation

India is the world's largest remittance recipient (~$120B/year, World Bank 2024).
Most on-chain stablecoins are USD-pegged, forcing Indian users to convert:

```
USD remittance → USDT (USD-peg) → INR (conversion loss + wait)
```

With ZINR:

```
ZBX Chain → ZINR (INR-peg) → recipient in India
No USD conversion. No FX spread. Instant settlement.
```

Additionally:
- PayID (ZEP-001) integration allows UPI-style payments in ZINR
- Bridge allows ZINR to flow to Ethereum/BSC/Polygon as ERC-20/BEP-20
- ZBX DeFi can offer INR-denominated lending, liquidity pools, and yield

---

## Specification

### Token parameters

| Parameter | Value |
|-----------|-------|
| **Name** | ZBX Indian Rupee |
| **Symbol** | ZINR |
| **Decimals** | 18 |
| **Peg** | 1 ZINR = 1 Indian Rupee |
| **Hard supply cap** | 10,000,000,000,000 ZINR (₹10 lakh crore) |
| **Default daily mint cap** | 1,000,000,000 ZINR/minter/day (₹100 crore) |
| **Transfer fee** | 0 bps default (governance-adjustable, max 1%) |
| **Contract type** | Built-in system contract (`zbx-contracts/zinr.rs`) |

### Features beyond ZUSD

| Feature | ZUSD | ZINR |
|---------|------|------|
| ERC-20 core (transfer, approve, allowance) | ✓ | ✓ |
| Mint / burn | ✓ | ✓ |
| burn_from (bridge) | ✓ | ✓ |
| Emergency pause | — | ✓ |
| Address blacklist (PMLA/FEMA compliance) | — | ✓ |
| Address freeze (court-order compliance) | — | ✓ |
| Per-minter daily mint caps | — | ✓ |
| Oracle address (peg attestation) | — | ✓ |
| Transfer fee (governance-set, 0–1%) | — | ✓ |
| Rich event types | — | ✓ |

---

## Peg Mechanism

ZINR maintains its peg via a reserve-backed collateral model:

```
User deposits INR (via licensed payment partner)
        ↓
Reserve custodian verifies + notifies oracle
        ↓
Oracle-authorized minter mints ZINR 1:1 to user
        ↓
Reserve holds INR in escrow (audited quarterly)
```

**Redemption**:
```
User burns ZINR → reserve releases INR to user's bank account
```

**Peg deviation circuit breaker**:
- If oracle detects ZINR/INR price deviation > 0.5%, oracle can immediately
  call `pause()` — no owner action needed.
- Only owner (governance multisig) can `unpause()`, preventing oracle griefing.

---

## Regulatory Compliance (India)

India's regulatory environment (PMLA 2002, FEMA 1999, RBI guidelines) requires:

### Blacklist — PMLA/FEMA
```rust
// Block a sanctioned or flagged address entirely
contract.blacklist(&owner, flagged_address)?;
// → cannot send OR receive ZINR
```

### Freeze — Court orders / ED attachment
```rust
// Freeze assets under court order
contract.freeze(&owner, suspect_address)?;
// → cannot send ZINR (can still RECEIVE — salary/refunds unaffected)
```

### Transfer fee — TDS compliance (future)
```rust
// Governance can set up to 1% fee
contract.set_transfer_fee_bps(&owner, 10)?; // 0.1%
// → deducted on every transfer, sent to fee_recipient
```

---

## Bridge Integration

ZINR is in the bridge whitelist with limits appropriate for India's remittance use:

| Parameter | Value |
|-----------|-------|
| **Max per transaction** | 10,000,000 ZINR (₹1 crore) |
| **Daily bridge limit** | 500,000,000 ZINR (₹50 crore/day) |
| **Bridge model** | Burn-and-Mint |
| **Supported chains** | Ethereum (ZINR ERC-20), BSC (ZINR BEP-20), Polygon |

**Burn-and-Mint flow**:
```
ZBX Chain:
  user approves bridge to burn ZINR
  bridge calls zinr.burn_from(bridge, user, amount)
  → ZINR destroyed on ZBX Chain

Target chain (Ethereum/BSC/Polygon):
  3-of-5 relayer multisig confirms
  Merkle receipt proof verified
  ZINR ERC-20 minted to recipient
```

---

## PayID Integration (ZEP-001)

ZINR works seamlessly with ZBX PayID addresses:

```
alice@zebvix  →  sends 1000 ZINR  →  bob@zebvix
```

This enables UPI-style payments in INR value:
- `send alice@zebvix 500 ZINR` = pay ₹500 instantly
- No bank account needed
- Irreversible settlement in ~5 seconds (1 ZBX block)

---

## Security Considerations

| Risk | Mitigation |
|------|------------|
| Oracle compromise | Oracle can only pause — cannot mint/burn/blacklist |
| Minter compromise | Per-minter daily cap limits damage; governance can remove minter instantly |
| Peg depeg | Oracle pause + governance vote to resolve |
| Regulatory seizure | Freeze (not blacklist) allows court orders without disrupting legitimate flow |
| Bridge drain | Daily bridge limit (₹5 crore/day) caps maximum daily outflow |
| Supply inflation | Per-minter daily caps + hard supply cap (₹10 lakh crore) |

---

## Differences from ZUSD

```
ZUSD: USD-peg, simple mint/burn, no compliance features
ZINR: INR-peg, full compliance suite, per-minter caps, oracle, pause, fee
```

ZUSD targets global DeFi. ZINR targets India's regulated financial system.
Both coexist as separate contracts.

---

## Implementation

**Contract**: `crates/zbx-contracts/src/zinr.rs`  
**Bridge**: `crates/zbx-bridge/src/token.rs` → `BridgeToken::zinr()`  
**ZEP**: `docs/proposals/ZEP-013-ZINR.md` (this file)  
**Tests**: 17 unit tests covering all features (run: `cargo test -p zbx-contracts`)

---

## Copyright

Copyright 2026 Zebvix Foundation. This ZEP is licensed under CC0.
