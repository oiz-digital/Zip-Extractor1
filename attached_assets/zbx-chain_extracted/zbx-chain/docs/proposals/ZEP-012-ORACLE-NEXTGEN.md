# ZEP-012: Next-Generation Oracle Suite

| Field        | Value                                                        |
|:-------------|:-------------------------------------------------------------|
| ZEP Number   | ZEP-012                                                      |
| Title        | Next-Generation Oracle Suite                                 |
| Status       | **IMPLEMENTED — Session 40** (TWAP + Circuit Breaker + DEX + Multi-Chain + Slasher + Heartbeat + Merkle Proof) |
| Category     | Core / Oracle Infrastructure                                 |
| Authors      | Zebvix Core Team                                             |
| Last Updated | 2026-05-05 (Session 40)                                      |

## Abstract

ZEP-012 extends ZEP-011 (basic Chainlink-style oracle) with advanced oracle
systems that go beyond what Chainlink, Pyth, or UMA provide today.

**Session 40 delivered 7 fully-implemented modules:**
1. **TWAP Oracle** — Ring-buffer time-weighted average price (`twap.rs`)
2. **Circuit Breaker** — Per-feed Closed/Open/Half-Open FSM (`circuit_breaker.rs`)
3. **DEX Fetcher** — On-chain DEX price sources with TVL-weighted TWAP (`dex_fetcher.rs`)
4. **Multi-Chain Relay** — 8-network oracle relay via ZBX-XCM (`multi_chain.rs`)
5. **Reporter Slasher** — 4-tier economic slashing for bad reporters (`slasher.rs`)
6. **Heartbeat Monitor** — Per-feed health and stale detection (`heartbeat.rs`)
7. **Merkle Price Proof** — Block-header price commitments (`proof.rs`)

Items still on the roadmap: ZK-Oracle (ZEP-012a), Optimistic Oracle (ZEP-012c), AI Anomaly Guard (ZEP-012d).

---

## Comparison with Existing Solutions (Session 40 updated)

| Feature | Chainlink | Pyth | UMA | **ZBX (ZEP-012)** |
|:--------|:----------|:-----|:----|:------------------|
| Price feeds | ✅ | ✅ | ❌ | ✅ **14 feeds** |
| On-chain TWAP (ring buffer) | ❌ | ❌ | ❌ | ✅ **Implemented** (4 windows) |
| Circuit breaker FSM | Partial | Partial | ❌ | ✅ **Implemented** (velocity + bounds) |
| DEX price sources | ❌ | Partial | ❌ | ✅ **Implemented** (3 protocols) |
| Multi-chain relay | ✅ (push) | ✅ (pull) | ❌ | ✅ **8 EVM networks** |
| Reporter slashing (tiered) | Partial | ❌ | ✅ | ✅ **Implemented** (4 severity tiers) |
| Feed heartbeat monitor | ✅ | ✅ | ❌ | ✅ **Implemented** (per-feed) |
| Merkle price commitment | ❌ | ❌ | ❌ | ✅ **Novel** (block header proof) |
| ZK-proven sources | ❌ | ❌ | ❌ | 🗓 Roadmap (ZEP-012a) |
| Arbitrary data (optimistic) | ❌ | ❌ | ✅ | 🗓 Roadmap (ZEP-012c) |
| AI manipulation detection | ❌ | ❌ | ❌ | 🗓 Roadmap (ZEP-012d) |
| Flash loan resistance | Partial | Good | Good | ✅ **Best** (TWAP + circuit breaker) |

---

## TWAP Oracle — Implemented (`twap.rs`)

Ring buffer: **1,024 observations** per feed. Formula: `TWAP = Σ(price_i × Δt_i) / Σ(Δt_i)`

| Window | Blocks (5s block time) | Primary use case |
|:-------|:----------------------|:-----------------|
| 5 min | 60 blocks | DEX arbitrage signals |
| 30 min | 360 blocks | Lending collateral check (default) |
| 2 hour | 1,440 blocks | Options / perps settlement |
| 24 hour | 17,280 blocks | Index rebalancing |

`TwapRegistry` manages one `TwapBuffer` per feed. Usage:

```rust
let registry = TwapRegistry::new();
registry.record(FeedId::Sol, price_u128, timestamp_secs);
let twap_30m = registry.twap(FeedId::Sol, TwapWindow::ThirtyMin)?;
```

### Flash-Loan Manipulation Cost

```
Cost = move_pct × TVL × (window_hours / 24) × annual_capital_rate

$100M TVL, 5% move, 30-min window, 10% APY:
  Cost ≈ 0.05 × $100M × (0.5/24) × 0.10 ≈ $10,417 per attack round
```

---

## Circuit Breaker — Implemented (`circuit_breaker.rs`)

Per-feed Finite State Machine:

```
[Closed] ──trip──► [Open] ──cooldown(5 min)──► [Half-Open] ──3 clean rounds──► [Closed]
                                                              ──bad round──► [Open]
```

**Trip conditions:**

| Condition | Type | Threshold |
|:----------|:-----|:----------|
| Price < `min_answer` | Absolute | Feed-specific |
| Price > `max_answer` | Absolute | Feed-specific |
| Single-round price move | Velocity | >20% standard / >5% stablecoins |
| Reporters < quorum | Quorum | `min_reporters` |
| No update > heartbeat + grace | Stale | Heartbeat + 5 min |

`BreakerRegistry` manages all feeds. Tripped feeds return last valid price with stale flag.

---

## DEX Fetcher — Implemented (`dex_fetcher.rs`)

On-chain DEX prices as a secondary oracle source:

| Protocol | Network | Math | TVL weight |
|:---------|:--------|:-----|:----------|
| Uniswap V3 | Ethereum | `sqrtPriceX96` → USD | Highest |
| PancakeSwap V3 | BSC | `sqrtPriceX96` → USD | High |
| ZBX DEX | ZBX Chain | ZEP-014 CLAMM | Bootstrapping |

`sqrtPriceX96` conversion: `price = (sqrtPriceX96 / 2^96)^2 × decimals_ratio`

DEX prices are:
- TVL-weighted across protocols
- TWAP-gated (30-min window) — single-block flash loan has zero effect
- Weighted lower than CEX sources to prevent on-chain-only manipulation

---

## Multi-Chain Relay — Implemented (`multi_chain.rs`)

**8 EVM networks** receive ZBX oracle prices via ZBX-XCM (ZEP-026):

| # | Network | Chain ID | Relay contract |
|:--|:--------|:---------|:--------------|
| 1 | ZBX Mainnet | **8989** | Native |
| 2 | ZBX Testnet | **8990** | Native |
| 3 | Ethereum | 1 | `ZbxAggregatorETH.sol` |
| 4 | BNB Smart Chain | 56 | `ZbxAggregatorBSC.sol` |
| 5 | Polygon | 137 | `ZbxAggregatorPoly.sol` |
| 6 | Arbitrum One | 42,161 | `ZbxAggregatorArb.sol` |
| 7 | Optimism | 10 | `ZbxAggregatorOP.sol` |
| 8 | Avalanche C-Chain | 43,114 | `ZbxAggregatorAvax.sol` |

`RelayMessage` carries a **96-byte BLS12-381 aggregate signature** over the oracle committee.
`MultiChainRegistry` detects stale relays (> 2× heartbeat since last update).

All relay contracts implement `AggregatorV3Interface` — Chainlink-compatible.

---

## Reporter Slasher — Implemented (`slasher.rs`)

4-tier economic slashing on top of ZEP-011 stake requirement (100 ZBX):

| Consecutive outlier rounds | Severity | Slash amount |
|:--------------------------|:---------|:------------|
| 1 | Warning | None (log only) |
| 2 | Minor | 5% of bonded stake |
| 3+ | Major | 10% of bonded stake |
| ≥2 reporters, same wrong price | Critical | 30% stake + suspension |

`SlashRecord` stored per reporter. Appeal window: **1,440 blocks** (~2 hours).
Governance multisig can reverse any slash within the appeal window.

---

## Heartbeat Monitor — Implemented (`heartbeat.rs`)

`HeartbeatMonitor` tracks last update per feed:

| State | Threshold | Action |
|:------|:----------|:-------|
| Fresh | < 75% of heartbeat | Normal operation |
| Warning | 75% – 100% of heartbeat | Log alert (`ORACLE_WARN`) |
| Critical | At heartbeat | `PEG_ALERT` event emitted |
| Stale | Heartbeat + 5 min grace | Circuit breaker trips |

Covers all 14 feeds with feed-specific heartbeat intervals.

---

## Merkle Price Proof — Implemented (`proof.rs`)

Each oracle round produces a Merkle commitment:

```
Leaf_i  = keccak256(feed_id ‖ price ‖ round_id ‖ timestamp)
Root    = BinaryMerkleRoot(sorted leaves)  // alphabetical by feed_id
```

`oracle_price_root` written to ZBX **block header**. Enables:
- Light clients: verify any single feed with **O(log 14) ≈ 4 hashes**
- Cross-chain relays: include price proofs in `RelayMessage`
- Off-chain indexers: audit historical price commitments without trusting node

`CommitmentRegistry` stores rolling history. `PriceProof::verify()` is O(log n).

---

## Oracle Priority Stack (Session 40)

```
1. ZK-Oracle (ZEP-012a — roadmap)              ← cryptographic source proof
2. Circuit-validated CEX oracle (ZEP-011 S40)  ← 8 sources, breaker, slasher
3. TWAP (30-min fallback)                       ← ring buffer, flash-loan safe
4. DEX TWAP (secondary)                         ← TVL-weighted, on-chain
5. Optimistic Oracle (ZEP-012c — roadmap)       ← historical / arbitrary data
```

---

## Implementation Status

| Component | Status | Module | Session |
|:----------|:-------|:-------|:--------|
| TWAP ring buffer (4 windows) | ✅ Implemented | `twap.rs` | S40 |
| Circuit breaker FSM | ✅ Implemented | `circuit_breaker.rs` | S40 |
| DEX price fetcher (3 protocols) | ✅ Implemented | `dex_fetcher.rs` | S40 |
| Multi-chain relay (8 networks) | ✅ Implemented | `multi_chain.rs` | S40 |
| Reporter slasher (4 tiers) | ✅ Implemented | `slasher.rs` | S40 |
| Heartbeat monitor (14 feeds) | ✅ Implemented | `heartbeat.rs` | S40 |
| Merkle price proof | ✅ Implemented | `proof.rs` | S40 |
| ZK-Oracle (Groth16 price proof) | 🗓 Roadmap | — | ZEP-012a |
| Optimistic Oracle (UMA-style) | 🗓 Roadmap | — | ZEP-012c |
| AI Anomaly Guard (0xCA) | 🗓 Roadmap | — | ZEP-012d |

**Build result**: `Finished dev profile [optimized + debuginfo] 0 errors` ✓

---

## ZK-Oracle — Roadmap (ZEP-012a)

Gives a cryptographic guarantee that prices came from a valid signed TLS response:

- Private inputs: TLS session transcript, Notary sig, CEX API key
- Public inputs: symbol_hash, price, timestamp, vk_hash
- Proof: Groth16 (BN254) — ~280k gas to verify on ZBX EVM
- Target: Block 400,000

---

## Optimistic Oracle — Roadmap (ZEP-012c)

Enables ZBX DeFi to use ANY real-world data (options expiry, prediction markets,
insurance triggers, governance data).

- Challenge window: 2 hours
- Bond: 50 ZBX (~$125 at $2.50)
- DVM voting: 72-hour commit-reveal by ZBX stakers
- Target: Block 300,000

---

## AI Anomaly Guard — Roadmap (ZEP-012d)

On-chain AI inference via `0xCA` precompile:

| Score | Action |
|:------|:-------|
| < 0.30 | Accept — normal |
| 0.30–0.70 | Accept + log `OracleSuspicious` |
| 0.70–0.95 | Reject → use TWAP |
| > 0.95 | Emergency pause |

Target: Block 500,000 (after AI precompile ZEP-009).
