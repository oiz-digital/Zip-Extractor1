# ZUSD — Zebvix Native Stablecoin

**Symbol**: ZUSD  
**Peg**: $1.00 USD  
**Type**: Collateral-backed (CDP model)  
**Collateral**: ZBX  
**Contracts**: ZUSD.sol, ZusdVault.sol, ZusdStabilityPool.sol, ZusdPricePeg.sol

---

## Overview

ZUSD is Zebvix Chain's native stablecoin. Unlike USDT/USDC which depend on external issuers, ZUSD is **created on-chain** by locking ZBX collateral.

```
User locks ZBX  →  ZusdVault mints ZUSD  →  User spends ZUSD
User repays ZUSD  →  ZusdVault burns ZUSD  →  ZBX returned
```

---

## Key Parameters

| Parameter | Value | Why |
|-----------|-------|-----|
| Min collateral ratio | 200% | Max mint = 50% of ZBX collateral value |
| Liquidation threshold | 100% | 50% price drop → instant liquidation |
| Liquidation bonus | 10% | Incentive for liquidators |
| Stability fee | 2% APY | Borrowing cost (burned = deflationary) |
| Redemption fee | 0.5% | Floor mechanism to maintain $1 peg |
| Min mint | 100 ZUSD | Avoid dust CDPs |

---

## How to Get ZUSD

### As a User — Mint ZUSD
```
Lock 10,000 ZBX (worth $5,000)
→ Mint up to 2,500 ZUSD (50% of $5000)
→ Use ZUSD in DeFi
→ Pay back anytime to reclaim ZBX

Safety rule: if ZBX price drops 50%, position liquidates.
→ Always keep buffer: mint 30-40% max for safety
```

### As a Trader — AMM

```
ZBX/ZUSD pool (genesis pool, 0.30% fee)
→ Swap ZBX for ZUSD directly
→ No collateral needed
→ Multi-hop routes via WZBX for ERC-20 tokens (router finds best path automatically)
```

**Canonical AMM pool involving ZUSD** (ZEP-014):

| Pool | Fee | Starting price |
|------|-----|----------------|
| ZBX/ZUSD | 0.30% | 1 ZUSD = 20 ZBX |

---

## Liquidation Rule (CR-only)

Liquidation triggers when collateral ratio falls to **100% or below**.

| Condition | Rule |
|-----------|------|
| **Price** | ZBX dropped 50%+ from open price (CR ≤ 100%) |

> **Note (vault v0.2):** A previously-documented "wallet-aware" carve-out
> (gating liquidation on the borrower's ZUSD balance) was removed in
> ZusdVault v0.2. It was unsound — a borrower could permanently dodge
> liquidation by *holding* (not burning) ZUSD while the protocol carried
> the price risk on an undercollateralised position. Aave / Liquity /
> MakerDAO all gate solely on collateral ratio. Removed; this section
> reconciled with code in S15-P2 (audit S6-V2-FIXED).

**Protect yourself:**
```
1. Keep CR > 200% (mint < 50% of collateral value)
2. Add collateral when ZBX drops
3. Repay early when CR approaches 150%
4. Or use a keeper bot to auto-add collateral
```

## Peg Mechanisms

| Situation | Mechanism |
|-----------|-----------|
| ZUSD < $1 | **Redemption arbitrage** — buy ZUSD at $0.97, redeem for $1 ZBX (see Redemption section) |
| ZUSD > $1 | Open CDP, mint ZUSD, sell at >$1 — profit |
| ZBX crash | Stability pool absorbs liquidations |
| Emergency | `redemptionPaused` switch + new-CDP suspension |

---

## Redemption — The Peg Floor (S15-P2: re-enabled, S6-V2-FIXED)

ZUSD holders can **redeem** ZUSD directly with the protocol for `$1`-worth of
ZBX (minus a 0.5% fee). This is the lower-bound peg-defence mechanism.

### How a redeemer uses it

```solidity
// 1. Off-chain helper / SDK / explorer returns CDPs sorted ASCENDING by CR.
address[] memory hints = sdk.getCdpsAscByCr(20);  // top-20 lowest CR

// 2. Call vault.redeem with the sorted hint list.
(uint256 zusdRedeemed, uint256 zbxOut) =
    vault.redeem(1000e18, hints, 20);  // redeem 1000 ZUSD, max 20 CDPs
```

The vault verifies on-chain that hints are monotone-non-decreasing in CR
(ascending-CR ordering **within the supplied hints** — global lowest-CR-first
is a best-effort property of the off-chain SDK; mainnet adds an on-chain
sorted CDP linked list — see ZEP-005 §7 limitation 1), and returns
ZBX = redeemed/zbxPrice − 0.5% fee.

### Per-CDP impact (the S6-V2 bug fix)

When the vault redeems against a CDP it ATOMICALLY decrements both
`cdp.collateral` AND `cdp.debt` (the prior bug was that only the global
`totalDebt` was decremented; per-CDP records were left stale, silently
draining the vault). The redemption mathematically **increases** the
target CDP's collateral ratio when CR ≥ 100% pre-redemption — i.e. it is
equivalent to the borrower repaying their own debt at par.

### Safety properties

| Property | How enforced |
|---|---|
| Vault solvency | `Σ cdp.collateral + leftover_returns == ZBX_in_vault` (test invariant) |
| Ascending-CR WITHIN supplied hints | On-chain monotonicity check (see ZEP-005 §7 — global lowest-CR-first is best-effort via SDK on testnet; mainnet adds on-chain sorted list) |
| Healthy-only | `require(CR ≥ 100%)` per CDP — bad-debt CDPs go to `liquidate()` |
| No dust CDPs | Partial redemptions cap at `MIN_ZUSD_MINT` post-debt |
| Spam-resistant | `MIN_REDEEM_AMOUNT = 10 ZUSD`, `0.5% fee` |
| Gas-bounded | `MAX_REDEEM_ITER = 50` per call |
| Reentrancy-safe | `nonReentrant` modifier on `redeem()` |
| Emergency stop | Owner can flip `redemptionPaused` |

### Limits (testnet-grade, mainnet hardening tracked)

- **No on-chain sorted CDP list** — caller supplies hints; SDK helper required.
- **Single oracle source** — TWAP/multi-source upgrade tracked separately.
- **No multi-call splitting** — large redemptions need multiple txs (capped at 50 CDPs each).

See `docs/proposals/ZEP-005-ZUSD-REDEMPTION.md` for full design + invariants.

---

## Stability Pool — Earn from Liquidations

Deposit ZUSD, earn ZBX:

```
Deposit 10,000 ZUSD into StabilityPool
→ When CDPs liquidate, pool absorbs debt
→ Pool receives ZBX collateral (+ 10% bonus)
→ You earn proportional ZBX share
→ Net return: ~5-15% APY in ZBX (varies with liquidation volume)
```

---

## Genesis Launch Plan

At mainnet launch (block 1), ZUSD comes alive through:

1. **Pre-mint**: Foundation Treasury receives 100,000,000 ZUSD genesis pre-mint (ZEP-014 §Genesis Seeding)
2. **AMM seeding**: 1,000,000 ZUSD + 20,000,000 ZBX → ZBX/ZUSD pool (0.30% fee)
3. **Community CDPs**: Users lock ZBX → mint ZUSD at 200%+ CR
4. **Peg anchor**: ZBX/ZUSD AMM + oracle feed (ZEP-011 ZUSD/USD feed, 5 reporters, 30s heartbeat)

**Genesis AMM pool involving ZUSD:**
```
ZBX/ZUSD:   20,000,000 ZBX + 1,000,000 ZUSD → starting price 1 ZUSD = 20 ZBX
```

**ZUSD peg defence (two-sided):**
- Upper bound ($1+): open CDP, mint ZUSD, sell at >$1 → arbitrage closes spread
- Lower bound ($1−): buy ZUSD at $0.97, redeem for $1 ZBX via `vault.redeem()` (ZEP-005)