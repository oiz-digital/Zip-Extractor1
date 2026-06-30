# ZEP-044: On-Chain Options Market

| Field       | Value                                           |
|-------------|-------------------------------------------------|
| ZEP         | ZEP-044                                         |
| Title       | European Put/Call Options — Oracle Cash Settlement |
| Author      | Zebvix Core Team                                |
| Status      | IMPLEMENTED                                     |
| Category    | Trading / DeFi                                  |
| Created     | 2026-05-05                                      |
| Contracts   | ZbxOptions.sol                                  |
| Depends On  | ZbxOracle.sol / ZbxTwapOracle.sol               |

---

## Abstract

ZEP-044 introduces a trustless European options market on ZBX Chain. Writers (sellers) create option series, post collateral, and receive premium upfront. Buyers pay the premium and hold the right to exercise at expiry. Settlement is cash-based — the oracle price determines payoff automatically.

---

## Motivation

Options are the most important derivative instrument in finance, enabling hedging, income generation (covered calls), and directional speculation with defined risk. On-chain options are currently rare on EVM chains due to complexity. ZBX Chain's native oracle infrastructure makes a fully on-chain, non-custodial options market feasible.

---

## Options 101

| Term | Meaning |
|------|---------|
| **Call** | Right to "buy" — profitable when price rises above strike |
| **Put** | Right to "sell" — profitable when price falls below strike |
| **Strike** | The agreed reference price |
| **Premium** | Upfront cost to buy the option |
| **Expiry** | Date/time when settlement occurs |
| **Writer** | Sells the option, posts collateral, receives premium |
| **Buyer** | Pays premium, holds exercise right |
| **ITM** | In-The-Money: payoff > 0 |
| **OTM** | Out-of-The-Money: payoff = 0 |
| **European** | Can only exercise at expiry (not before) |

---

## Payoff Formulas

```
CALL payoff = max(0, settlementPrice − strikePrice)  per contract
PUT  payoff = max(0, strikePrice − settlementPrice)  per contract

cashPayoff = payoff × contracts / 1e18
             (capped at collateralPerContract × contracts / 1e18)
```

**Example:**
- ZBX CALL, strike = 1.00 ZUSD, 100 contracts
- Settlement price = 1.35 ZUSD
- Payoff = (1.35 − 1.00) × 100 = 35 ZUSD (if collateral covers)

---

## Specification

### Option Series

Writer calls `writeSeries()` to create a series:

```solidity
struct OptionSeries {
    address writer;
    address oracle;
    address collateralToken;   // e.g. ZUSD
    bool    isCall;
    uint256 strikePrice;       // 18-decimal
    uint256 expiry;
    uint256 contracts;         // total written (1e18 = 1 contract)
    uint256 contractsSold;
    uint256 premium;           // collateral per 1e18 contracts
    uint256 collateralPerContract;  // = strikePrice (v1 cap)
    uint256 settlementPrice;   // locked at expiry
    bool    settled;
}
```

### Collateral Requirement (v1)

| Option Type | Collateral Per Contract |
|-------------|------------------------|
| CALL | `strikePrice` in collateral token (capped max payoff = 2× strike move) |
| PUT | `strikePrice` in collateral token (covers full downside to 0) |

### Series Lifecycle

```
writeSeries()           → writer posts collateral, series created
    ↓
buyOptions(id, amount)  → buyer pays premium; writer receives premium
    ↓ (at/after expiry)
settleSeries(id)        → anyone locks oracle settlement price
    ↓
exercise(id, buyer, n)  → pays cashPayoff to buyer if ITM
    ↓
writerWithdraw(id)      → writer reclaims unexercised collateral
```

### Fee

**0.5% of premium** collected at `buyOptions()`, sent to protocol treasury.

### Writer Revenue Model

Writer earns: full premium (minus 0.5% fee) received immediately.  
Writer risk: if option is ITM at expiry, collateral is used to pay buyers.  
Net P&L for writer:

```
If OTM: premium kept + collateral returned
If ITM: premium kept + collateral reduced by payoff amount
```

### Multi-Series Support

Multiple independent series can exist for the same underlying (different strikes/expiries). Each is self-contained with its own collateral pool.

---

## Use Cases

| Strategy | Action |
|----------|--------|
| Bull bet | Buy CALL (pay premium, profit if price rises) |
| Bear bet | Buy PUT (pay premium, profit if price falls) |
| Covered call | Hold ZBX + write CALL (earn premium, cap upside) |
| Protective put | Hold ZBX + buy PUT (pay premium, hedge downside) |
| Cash-secured put | Write PUT with ZUSD collateral (earn premium, buy ZBX cheaply if falls) |

---

## Security Considerations

| Risk | Mitigation |
|------|-----------|
| Writer collateral undersizing (CALL) | v1: capped at strikePrice; v2: dynamic strike × multiplier |
| Double settlement | `settled` flag — single settlement lock |
| Writer double-withdraw | `writerWithdrawn` flag |
| Exercise on OTM | `_calcPayoff` returns 0 → reverts `NothingToExercise` |
| Reentrancy on exercise | `bp.exercised` updated before `transfer` |
| Premium front-run | Premium immutable after `writeSeries()`; buyer checks before calling |

---

## Implementation

- **Contract:** `zbx-chain-extracted/zbx-chain/contracts/ZbxOptions.sol`
- **Key functions:** `writeSeries(...)`, `buyOptions(seriesId, amount)`, `settleSeries(seriesId)`, `exercise(seriesId, buyer, amount)`, `writerWithdraw(seriesId)`, `intrinsicValue(seriesId)`, `isITM(seriesId)`, `estimatePayoff(seriesId, amount)`, `withdrawFees(token)`

---

## Status

IMPLEMENTED — Session 47 (2026-05-05). Medium finding documented (CALL collateral cap in v1).
