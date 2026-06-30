# ZEP-011: Decentralized Price Oracle

| Field        | Value                                                       |
|:-------------|:------------------------------------------------------------|
| ZEP Number   | ZEP-011                                                     |
| Title        | Decentralized Price Oracle (Chainlink-style)                |
| Status       | **Active — Session 40 Advanced Oracle Upgrade**             |
| Category     | Core / DeFi Infrastructure                                  |
| Authors      | Zebvix Core Team                                            |
| Last Updated | 2026-05-05 (Session 40)                                     |

## Abstract

ZEP-011 introduces a native decentralized price oracle for ZBX chain.
Oracle nodes fetch prices from external CEXes and DEXes, aggregate via median/VWAP,
and publish on-chain via `ZbxAggregatorV3` smart contracts.
The interface is fully compatible with Chainlink's `AggregatorV3Interface`.

**Session 40 upgrade** extended from a 7-feed / 3-CEX / single-chain oracle to a
**14-feed / 8-source / 8-network** advanced oracle suite with TWAP ring buffers,
circuit breakers, DEX price fetching, reporter slashing, heartbeat monitoring,
Merkle price proofs, and cross-chain relay to 6 external EVM networks.

---

## Architecture (Session 40)

```
External Sources — Tier 1 (Primary CEX)
  Binance ───────┐
  Coinbase ──────┤
  Kraken ────────┤     ┌─────────────────────┐     ┌───────────────────┐
                 │     │   Oracle Node Pool   │     │ ZbxAggregatorV3   │
External Sources — Tier 2 (Secondary CEX)     ├────►│ latestRoundData() │
  Gate.io ───────┼────►│ fetch → VWAP         │     │ Chainlink-compat  │
  Bybit ─────────┤     │ IQR outlier removal  │     └────────┬──────────┘
  KuCoin ────────┤     │ circuit breaker      │              │ ZBX-XCM relay
                 │     │ median → sign        │              ▼ (ZEP-026)
External Sources — Tier 3 (Aggregators)        │     ┌────────────────────────────┐
  CoinGecko ─────┤     └─────────────────────┘     │ 6 relay chains:            │
  CMC ───────────┘                                  │ ETH · BSC · Polygon        │
                                                    │ Arbitrum · Optimism · Avax │
DEX Sources                                         └────────────────────────────┘
  Uniswap V3 ──┐
  PancakeSwap ─┼──► TVL-weighted TWAP feed
  ZBX DEX ─────┘
```

---

## Aggregation Pipeline (Session 40 — 10 steps)

Each oracle round:

1. Fetch prices from all 8 CEX/aggregator sources in parallel
2. Fetch DEX prices via `sqrtPriceX96` → USD math (Uniswap V3 / PancakeSwap / ZBX DEX)
3. Combine via VWAP (volume-weighted; Tier 1 highest weight)
4. IQR outlier removal (3× IQR fence)
5. Circuit breaker check — absolute min/max bounds + velocity guard (20%/round, 5% stablecoins)
6. Heartbeat check — reject if no update within configured interval
7. Median across reporter submissions (min_reporters threshold)
8. Merkle price commitment → `oracle_price_root` in block header
9. Write to `ZbxAggregatorV3` on-chain
10. Relay via ZBX-XCM (BLS-signed `RelayMessage`) to 6 external chains

---

## Supported Feeds — 14 Total (Session 40)

### Crypto Feeds (13)

| Feed | Update | Deviation | Heartbeat | Min Reporters | Circuit Breaker |
|:-----|:-------|:----------|:----------|:--------------|:----------------|
| ZBX/USD | 1 min | 0.5% | 1h | 5 | $0.01 – $1,000 |
| ZUSD/USD | 30 sec | 0.1% | 30m | 5 | $0.90 – $1.10 |
| ZNS/USD | 4h | 1.0% | 4h | 3 | $0.00001 – $1,000 |
| ETH/USD | 1 min | 0.5% | 1h | 5 | $1 – $1,000,000 |
| BTC/USD | 1 min | 0.5% | 1h | 5 | $1,000 – $10,000,000 |
| BNB/USD | 1 min | 0.5% | 1h | 5 | $1 – $1,000,000 |
| **SOL/USD** ← S40 | 1 min | 0.5% | 1h | 5 | $1 – $100,000 |
| **AVAX/USD** ← S40 | 1 min | 0.5% | 1h | 5 | $1 – $10,000 |
| **MATIC/USD** ← S40 | 2h | 1.0% | 2h | 3 | $0.001 – $1,000 |
| **ARB/USD** ← S40 | 2h | 1.0% | 2h | 3 | $0.001 – $10,000 |
| **OP/USD** ← S40 | 2h | 1.0% | 2h | 3 | $0.001 – $10,000 |
| **LINK/USD** ← S40 | 2h | 0.5% | 2h | 3 | $0.10 – $100,000 |
| **DOT/USD** ← S40 | 2h | 1.0% | 2h | 3 | $1 – $10,000 |

### Forex Feed (1)

| Feed | Update | Deviation | Heartbeat | Min Reporters | Sources |
|:-----|:-------|:----------|:----------|:--------------|:--------|
| USD/INR | 1h | 0.2% | 1h | 3 | RBI (10× weight), ExchangeRate-API, WazirX, CoinDCX, AI LLM (last-resort) |

---

## Price Sources — 8 CEX + Aggregators + DEX (Session 40)

| Tier | Source | Endpoint | Role |
|:-----|:-------|:---------|:-----|
| 1 | **Binance** | `api.binance.com/api/v3/ticker/price` | Primary (highest volume) |
| 1 | **Coinbase** | `api.coinbase.com/v2/prices/{pair}/spot` | Primary |
| 1 | **Kraken** | `api.kraken.com/0/public/Ticker` | Primary |
| 2 | **Gate.io** ← new | `api.gateio.ws/api/v4/spot/tickers` | Secondary |
| 2 | **Bybit** ← new | `api.bybit.com/v5/market/tickers` | Secondary |
| 2 | **KuCoin** ← new | `api.kucoin.com/api/v1/market/stats` | Secondary |
| 3 | **CoinGecko** ← new | `api.coingecko.com/api/v3/simple/price` | Validation cross-check |
| 3 | **CoinMarketCap** ← new | `pro-api.coinmarketcap.com/v2/quotes/latest` | Validation cross-check |
| DEX | **Uniswap V3** | Ethereum, on-chain `sqrtPriceX96` | DEX TWAP |
| DEX | **PancakeSwap V3** | BSC, on-chain `sqrtPriceX96` | DEX TWAP |
| DEX | **ZBX DEX** | ZBX Chain, ZEP-014 CLAMM | DEX TWAP |

---

## Advanced Modules — Session 40 (`crates/zbx-oracle/src/`)

### TWAP (`twap.rs`) — Manipulation-resistant time-weighted average price

Ring buffer: 1,024 observations (rolling). Formula: `Σ(price_i × Δt_i) / Σ(Δt_i)`

| Window | Primary use | Flash-loan resistance |
|:-------|:-----------|:----------------------|
| 5 min | DEX arbitrage signals | Low |
| 30 min | Lending collateral (default) | Medium |
| 2 hour | Options / perps settlement | High |
| 24 hour | Index rebalancing | Very high |

### Circuit Breaker (`circuit_breaker.rs`) — Per-feed FSM

States: **Closed** (normal) → **Open** (tripped) → **Half-Open** (recovery) → **Closed**

Trip conditions:
- Price outside absolute bounds (`min_answer` / `max_answer`)
- Velocity: single-round move > 20% (5% for stablecoins)
- Reporter quorum not met
- Heartbeat expired

Cool-down: 5 min before entering Half-Open. 3 consecutive good rounds to re-close.

### DEX Fetcher (`dex_fetcher.rs`) — On-chain DEX prices

- `sqrtPriceX96` → USD price math (Uniswap V3 / PancakeSwap V3)
- TVL-weighted aggregation across protocols
- DEX weight lower than CEX to prevent single-block manipulation
- Combined with TWAP (30-min window) for flash-loan resistance

### Reporter Slasher (`slasher.rs`)

| Consecutive rounds | Severity | Slash |
|:------------------|:---------|:------|
| 1st outlier | Warning | 0 (log) |
| 2nd consecutive | Minor | 5% stake |
| 3rd+ consecutive | Major | 10% stake |
| Coordinated (≥2 reporters, same wrong price) | Critical | 30% stake + suspension |

Appeal window: 1,440 blocks (~2 hours).

### Heartbeat Monitor (`heartbeat.rs`)

| Feed group | Heartbeat | Warning threshold | Stale threshold |
|:-----------|:----------|:-----------------|:----------------|
| ZUSD/USD | 30 min | 22.5 min (75%) | 35 min |
| ZBX/ETH/BTC/SOL/AVAX | 1 hour | 45 min | 65 min |
| MATIC/ARB/OP/LINK/DOT | 2 hours | 90 min | 125 min |
| ZNS/USD, USD/INR | 4 hours | 3 hours | 4h 5m |

Stale feeds: flagged in `latestRoundData()` `answeredInRound` vs `roundId`.

### Merkle Price Proof (`proof.rs`)

Each round produces:
```
oracle_price_root = MerkleRoot(
  keccak256(feed_id ‖ price ‖ round_id ‖ timestamp)
  for each feed, sorted alphabetically
)
```

Stored in ZBX block header as `oracle_price_root`. Light clients verify single-feed
prices with O(log 14) ≈ 4-node proof — no full oracle state download needed.

---

## Multi-Chain Oracle Relay — 8 EVM Networks (ZEP-026, `multi_chain.rs`)

| # | Network | Chain ID | Finality | Status |
|:--|:--------|:---------|:---------|:-------|
| 1 | ZBX Chain Mainnet | **8989** | Instant | Native |
| 2 | ZBX Chain Testnet | **8990** | Instant | Native |
| 3 | Ethereum Mainnet | 1 | 12 blocks | XCM Relay |
| 4 | BNB Smart Chain | 56 | 15 blocks | XCM Relay |
| 5 | Polygon Mainnet | 137 | 128 blocks | XCM Relay |
| 6 | Arbitrum One | 42,161 | Optimistic | XCM Relay |
| 7 | Optimism Mainnet | 10 | Optimistic | XCM Relay |
| 8 | Avalanche C-Chain | 43,114 | Instant | XCM Relay |

Relay flow:
```
ZBX oracle finalises round
  → BLS-signed RelayMessage (96-byte aggregate sig over oracle committee)
    → ZBX-XCM dispatcher (ZEP-026 `multi_chain.rs`)
      → on-chain ZbxAggregator.sol verifies BLS sig
        → latestRoundData() live on destination chain
```

All relay contracts implement `AggregatorV3Interface`. Any Chainlink-compatible
protocol on these networks uses ZBX price data without code changes.

---

## Manipulation Resistance (Session 40 — full suite)

| Attack vector | Protection |
|:-------------|:----------|
| Single reporter lies | Median — outlier has zero effect |
| <50% reporters collude | Median still correct |
| Flash loan spot manipulation | TWAP 30-min window is primary reference |
| Rapid price pump/dump | Circuit breaker velocity guard (20%/round) |
| Out-of-bounds price | Circuit breaker absolute bounds |
| Stale data served | Heartbeat monitor + stale flag in round data |
| Reporter cartel (coordinated) | Slasher Critical — 30% stake slash |
| Merkle commitment forgery | `keccak256` Merkle tree in block header |
| Cross-chain relay forgery | BLS 96-byte aggregate signature over committee |
| Single DEX manipulation | DEX weight < CEX; TWAP damping |

---

## INR Feed Architecture (unchanged from Session 25)

```
USD/INR — 5-source VWAP
  Source 1: RBI reference rate        weight = 10,000,000 (10× official anchor)
  Source 2: ExchangeRate-API          weight =  1,000,000
  Source 3: WazirX  USDT/INR          weight =    500,000
  Source 4: CoinDCX USDT/INR          weight =    300,000
  Source 5: AI LLM (last-resort)      weight =     50,000 (₹50–₹150 range guard)
```

3-tier fallback: Live VWAP → 30-day stale cache → Hard error (`AllSourcesFailedNoCache`).

---

## Contract Addresses (ZBX Mainnet — Chain ID 8989)

| Feed | Address |
|:-----|:--------|
| ZBX/USD | `0xFeed000100000000000000000000000000000001` |
| ZUSD/USD | `0xFeed000200000000000000000000000000000002` |
| ETH/USD | `0xFeed000300000000000000000000000000000003` |
| BTC/USD | `0xFeed000400000000000000000000000000000004` |
| USD/INR | `0xFeed000500000000000000000000000000000005` |
| BNB/USD | `0xFeed000600000000000000000000000000000006` |
| ZNS/USD | `0xFeed000700000000000000000000000000000007` |
| SOL/USD | `0xFeed000800000000000000000000000000000008` |
| AVAX/USD | `0xFeed000900000000000000000000000000000009` |
| MATIC/USD | `0xFeed000a0000000000000000000000000000000a` |
| ARB/USD | `0xFeed000b0000000000000000000000000000000b` |
| OP/USD | `0xFeed000c0000000000000000000000000000000c` |
| LINK/USD | `0xFeed000d0000000000000000000000000000000d` |
| DOT/USD | `0xFeed000e0000000000000000000000000000000e` |

---

## Chainlink Compatibility

```solidity
// Works on ZBX mainnet AND all 6 relay chains without code changes:
AggregatorV3Interface solFeed = AggregatorV3Interface(0xFeed0008...);
(, int256 price,,,) = solFeed.latestRoundData();
// price = 17000000000 → $170.00 (8 decimals)
```

---

## Reporter Incentives & Slashing

| Item | Value |
|:-----|:------|
| Reward per round | 0.001 ZBX (from oracle treasury) |
| Treasury source | 5% of oracle consumer fees |
| Registration stake | 100 ZBX |
| Minor slash (2nd consecutive outlier) | 5% stake |
| Major slash (3rd+ consecutive outlier) | 10% stake |
| Critical slash (coordinated attack) | 30% stake + suspension |
| Appeal window | 1,440 blocks (~2 hours) |

---

## Integration with ZBX DeFi

| Consumer | Feed(s) used | Purpose |
|:---------|:------------|:--------|
| ZUSD minting | ZBX/USD | Collateral ratio check |
| zbx-lending | ETH/USD, BTC/USD | Liquidation trigger (< 110%) |
| zbx-pool AMM | ZBX/USD | Deviation guard on swaps (>15% reverts) |
| zbx-confidential | ZBX/USD | Public reference price for ZK commitments |
| AI precompile (0xCA) | ZBX/USD | `ZusdRiskModel` collateral scoring |
| zbx-staking slasher | ZBX/USD | USD-denominated slash amount calculation |
