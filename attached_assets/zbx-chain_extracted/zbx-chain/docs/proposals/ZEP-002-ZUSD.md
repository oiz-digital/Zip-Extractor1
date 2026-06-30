# ZEP-002: ZUSD — Native Overcollateralized Stablecoin

> **⚠️ PARTIALLY SUPERSEDED (vault v0.2, S15-P2 / 2026-05-01)**
>
> The deployed ZUSD contracts have evolved past this proposal in two areas:
>
> 1. **Wallet-aware liquidation REMOVED** — the §"Wallet-Aware Liquidation
>    Protection" section below describes a v0.1 behaviour that was removed
>    in vault v0.2 (see `ZusdVault.sol:296-311` and `docs/ZUSD.md:60-74`).
>    Liquidation now gates **solely** on collateral ratio, aligned with
>    Aave / Liquity / MakerDAO. Reason: holding (not burning) ZUSD let
>    borrowers dodge liquidation indefinitely, leaving the protocol
>    carrying the price risk on undercollateralised positions.
>
> 2. **Redemption mechanism added** — see `ZEP-005-ZUSD-REDEMPTION.md` for
>    the hint-based, monotonicity-checked, atomic-per-CDP redemption that
>    closes the original S6-V2 bug and provides the lower-bound peg
>    defence missing from this proposal.
>
> All other parameters (200% min CR, 100% liquidation, 0.5% stability
> fee, 100k ZUSD/block mint cap) remain authoritative.

| Field         | Value                                   |
|---------------|-----------------------------------------|
| **ZEP**       | 002                                     |
| **Title**     | ZUSD — ZBX Native Overcollateralized Stablecoin |
| **Author**    | Zebvix Core Team                        |
| **Status**    | DEPLOYED (partially superseded — see banner above) |
| **Category**  | Standard                                |
| **Activation**| Block 1 (Genesis)                       |
| **Peg**       | 1 ZUSD = 1 USD                          |
| **Superseded by (partial)** | ZEP-005 (redemption), vault v0.2 (liquidation rule) |

---

## Summary

ZUSD ek **overcollateralized stablecoin** hai ZBX Chain pe, collateral ZBX tokens se.

Mechanism: User ZBX lock karta hai → ZUSD mint karta hai → ZBX unlock karne ke liye ZUSD burn karna padta hai.

---

## Key Parameters

| Parameter                  | Value                     |
|----------------------------|---------------------------|
| Minimum Collateral Ratio   | 200% (sirf 50% mint hoga) |
| Liquidation Threshold      | 100% CR (50% price drop)  |
| Liquidation Protection     | ~~Wallet-aware~~ → collateral-ratio only (vault v0.2 — see banner) |
| Stability Fee              | 0.5% annual               |
| Mint cap per block         | 100,000 ZUSD              |

---

## ~~Wallet-Aware Liquidation Protection~~ (REMOVED in vault v0.2)

> **Historical — this design was REMOVED in vault v0.2.** Original spec retained below for reference. Current vault implementation gates liquidation solely on collateral ratio (see banner at top + `ZusdVault.sol:296-311`).

ZEP-002 mein originally ek special protection tha:

```
Liquidation sirf tab hogi jab:
  1. Collateral ratio < 100% (ZBX price 50%+ gira)
  AND
  2. Owner ke wallet mein current ZUSD balance < outstanding debt
```

**Why removed (vault v0.2):** Borrowers could dodge liquidation indefinitely by *holding* (not burning) ZUSD, leaving the protocol carrying the price risk on undercollateralised CDPs. Aave / Liquity / MakerDAO all gate solely on collateral ratio for the same reason. Current behaviour: any CDP with CR ≤ 100% is liquidatable regardless of owner wallet balance.

---

## Contracts

- `ZUSD.sol` — ERC-20 stablecoin token
- `ZusdVault.sol` — CDP vault (mint/burn/liquidate)
- `ZusdStabilityPool.sol` — Liquidation buffer pool
- `ZusdPricePeg.sol` — Price oracle & peg mechanism