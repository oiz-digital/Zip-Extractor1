# MEV Protection on ZBX Chain

**Version**: 0.2  
**Crate**: zbx-mev  
**Status**: Production

---

## What is MEV?

Maximal Extractable Value (MEV) is profit extracted by block producers through transaction ordering manipulation:

- **Sandwich attacks**: Bot sees your DEX swap → inserts buy before, sell after → you get worse price
- **Frontrunning**: Bot copies your profitable tx and submits it first
- **Backrunning**: Bot immediately follows your tx for arbitrage profit

---

## ZBX Chain MEV Protection Layers

### Layer 1 — Private Mempool

Submit transactions encrypted. Block builders see only the hash until block sealing:

```bash
# Standard submission (public, frontrunnable)
zbx_sendTransaction { ... }

# Private submission (encrypted, frontrun-protected)
zbx_sendPrivateTransaction {
  "tx": "0x...",         # signed tx
  "maxBlockNumber": 1000 # cancel if not included by block 1000
}
```

### Layer 2 — Commit-Reveal Ordering

1. Block N: User submits H(tx) — the hash (no content visible)
2. Block N+1: User submits full tx — now included in order of commit time

Even if an attacker sees your commit, they don't know what you're doing.

### Layer 3 — Proposer-Builder Separation (PBS)

```
Validator (Proposer)       Block Builder
       │                        │
       │─── Slot auction ──────▶│
       │                        │ Builds block: selects bundles + txs
       │◀── Bid + block root ───│
       │
       │ Signs header (commits to builder's block)
       │────────────────────────▶ Builder reveals full block
```

Validators earn only through the auction, not by frontrunning users.

### Layer 4 — MEV Redistribution

Captured MEV (from searcher bundles) is redistributed:

| Recipient | Share |
|-----------|-------|
| Stakers (proportional to stake) | 30% |
| Governance treasury | 50% |
| Burned (deflationary) | 20% |

---

## For DApp Developers

To protect your users from MEV:

1. **Use commit-reveal** for sensitive operations (auctions, lotteries)
2. **Set slippage limits** in DEX swaps (ZbxRouter has built-in protection)
3. **Use time-weighted prices** (TWAP from ZbxAMM) instead of spot price
4. **Submit via private pool** for large trades

---

## For Searchers (Arbitrageurs / Liquidators)

Submit MEV bundles via the PBS relay:

```bash
curl -X POST https://relay.zbvix.com/relay/v1/builder/blocks \\
  -H "Content-Type: application/json" \\
  -d '{
    "txs": ["0x..."],
    "target_block": 100000,
    "builder_tip": "1000000000000000000"
  }'
```