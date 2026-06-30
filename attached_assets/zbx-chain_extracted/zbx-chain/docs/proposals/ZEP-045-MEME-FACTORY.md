# ZEP-045: Meme Coin Launchpad (ZbxMeme)

| Field       | Value                                             |
|-------------|---------------------------------------------------|
| ZEP         | ZEP-045                                           |
| Title       | Meme Coin Launchpad — pump.fun-style Fair Launch  |
| Author      | Zebvix Core Team                                  |
| Status      | IMPLEMENTED                                       |
| Category    | Standard / Meme / DeFi                            |
| Created     | 2026-05-05                                        |
| Contracts   | ZbxMemeFactory.sol, ZbxMemeToken.sol              |

---

## Abstract

ZEP-045 defines two complementary contracts for meme coin creation on ZBX Chain:

1. **ZbxMemeFactory** — pump.fun-style bonding curve launchpad. Anyone launches a meme coin in one transaction. Price auto-discovers via constant-product virtual reserves. When the market cap threshold is hit, liquidity is permanently added to ZbxAMM and LP tokens are burned — no rug possible.

2. **ZbxMemeToken** — standalone advanced meme coin with configurable buy/sell/transfer taxes, holder reflection, auto-burn, anti-whale, anti-snipe, and owner-renounce.

---

## Motivation

Meme coins are the highest-traffic use case on EVM chains. Pump.fun generated >$500M in fees in 2024. ZBX Chain needs a native, fair, trustless meme launchpad to capture this activity and bring liquidity on-chain.

---

## ZbxMemeFactory Specification

### Bonding Curve Model (Virtual Reserves)

Uses the same **constant-product formula** as a traditional AMM, but with **virtual** reserves:

```
k = virtualZbx × virtualToken  (fixed constant)

virtualZbx   = 30 ZBX   (virtual starting liquidity)
virtualToken = 1B tokens (all tokens start virtual)
```

**Buy (ZBX → tokens):**
```
tokensOut = virtualToken × zbxIn / (virtualZbx + zbxIn)
```

**Sell (tokens → ZBX):**
```
zbxOut = virtualZbx × tokenAmount / (virtualToken + tokenAmount)
```

**Price at any point:**
```
price = virtualZbx / virtualToken  [ZBX per token]
```

### Price Discovery Example

| Tokens Sold | Price (ZBX per 1M tokens) | Market Cap |
|------------|--------------------------|------------|
| 0          | 0.000030                 | $0         |
| 100M (10%) | 0.000033                 | ~$3.3      |
| 500M (50%) | 0.000060                 | ~$30       |
| 800M (80%) | 0.000150                 | ~$120      |
| Graduation | ~800M circulating        | 30 ZBX     |

### Launch Fee

**0.01 ZBX** to launch — prevents spam.

### Trade Fee

**1% per trade** (buy and sell). Collected as protocol revenue.

### Graduation

When `realZbxRaised >= 30 ZBX`:

1. Creator receives **1% of graduation ZBX**
2. Protocol receives **1% of graduation ZBX**
3. Remaining ZBX + remaining tokens → `ZbxAMM.addLiquidity()`
4. LP tokens sent to `0x000...dEaD` (**permanently burned**)
5. Bonding curve sealed — all future trades via ZbxAMM

**After graduation:** token price discovered by ZbxAMM (Uniswap v2 model). No more bonding curve trading.

### Anti-Rug Guarantees

| Mechanism | Implementation |
|-----------|---------------|
| No creator allocation | 100% of supply on bonding curve |
| LP permanently locked | LP tokens sent to dead address |
| Curve sealed post-grad | `graduated = true` → all buy/sell revert |
| Anti-snipe window | First 5 blocks: max 0.5% per TX |
| No admin token withdrawal | Factory holds tokens in curve only |

### Social Features

On-chain social engagement:
- `comment(memeId, text)` — stores comment as event
- `like(memeId)` — stores like signal as event

Explorer indexes these events to show a real-time comment feed and like count for each meme.

### View Functions

| Function | Returns |
|----------|---------|
| `currentPrice(memeId)` | Current token price in ZBX (18-dec) |
| `marketCap(memeId)` | Circulating supply × price |
| `quoteBuy(memeId, zbxAmount)` | Expected tokensOut + new price |
| `quoteSell(memeId, tokenAmount)` | Expected zbxOut + new price |
| `graduationProgress(memeId)` | 0–10000 bps toward graduation |
| `listMemes(offset, limit)` | Paginated meme list (newest first) |

---

## ZbxMemeToken Specification (Advanced Standalone)

For creators who want full control over their token's economics without the bonding curve.

### Tokenomics Features

| Feature | Detail |
|---------|--------|
| Buy tax | Configurable: burn% + reflect% + dev% (max 25% total) |
| Sell tax | Configurable: burn% + reflect% + dev% (max 25% total) |
| Transfer tax | Configurable (max 10% total) |
| Auto-burn | Portion of tax sent to 0xdead permanently |
| Reflection | Portion distributed to all holders proportionally |
| Dev wallet | Portion sent to dev wallet (treasury income) |

**Default tax:**
- Buy: 2% (1% burn + 1% reflect)
- Sell: 4% (2% burn + 2% reflect)
- Transfer: 0%

### Reflection Model

Uses the rToken (reflected token) accounting model:

```
rTotal = very large number ÷ totalSupply
rBalance[user] represents proportional share of rTotal

balanceOf(user) = rBalance[user] × tTotal / rTotal
```

When a reflection event occurs, `rTotal` decreases — every holder's `balanceOf` increases automatically without any transfer.

### Limits & Anti-Whale

| Limit | Default | Min |
|-------|---------|-----|
| Max wallet | 2% of supply | 1% |
| Max transaction | 1% of supply | 0.5% |
| Anti-snipe max TX | 0.5% of supply | N/A (fixed) |

### Anti-Snipe

First `SNIPE_BLOCKS = 3` blocks after `enableTrading()`:
- Max TX per transaction: **0.5% of supply**
- Bots that try to snipe at launch are limited to small buys

### Ownership Renounce

```solidity
renounceOwnership() → ownershipRenounced = true
```

Permanently removes all owner controls. Cannot be undone. Signals to community that the contract is fully decentralised. After renounce:
- No blacklist changes
- No tax changes
- No limit changes
- No trading enable/disable

---

## Security Considerations

### ZbxMemeFactory

| Risk | Mitigation |
|------|-----------|
| Flash loan price manipulation | Only ZBX accepted (no flash ZBX native token possible) |
| Graduation re-entry | `graduated = true` set before AMM call |
| Creator rug via allocation | Zero creator allocation — curve holds all tokens |
| LP lock bypass | LP tokens sent to dead address in same TX |
| Excessive snipe | Anti-snipe: 0.5% max TX for first 5 blocks |

### ZbxMemeToken

| Risk | Mitigation |
|------|-----------|
| Tax rug (>50%) | Hard cap: buy ≤ 25%, sell ≤ 25%, transfer ≤ 10% |
| Max wallet bypass | Check on each transfer post-move |
| Blacklist griefing | Renounce ownership to prevent future blacklisting |
| Reflection underflow | rTotal calculation uses full uint256 precision |
| Trading before liquidity | `enableTrading()` required; reverts with TradingNotEnabled |

---

## Implementation

- **ZbxMemeFactory:** `zbx-chain-extracted/zbx-chain/contracts/ZbxMemeFactory.sol`
  - Key functions: `launchMeme(...)`, `buy(memeId, minOut)`, `sell(memeId, amount, minZbxOut)`, `comment(memeId, text)`, `like(memeId)`, `currentPrice(...)`, `marketCap(...)`, `quoteBuy(...)`, `quoteSell(...)`, `graduationProgress(...)`, `listMemes(...)`

- **ZbxMemeToken:** `zbx-chain-extracted/zbx-chain/contracts/ZbxMemeToken.sol`
  - Key functions: `enableTrading()`, `setTax(...)`, `setMaxWallet(bps)`, `setMaxTx(bps)`, `setBlacklist(...)`, `setLiquidityPair(...)`, `renounceOwnership()`, `burn(amount)`

---

## Status

IMPLEMENTED — Session 48 (2026-05-05). 0 build errors. Security findings documented above.
