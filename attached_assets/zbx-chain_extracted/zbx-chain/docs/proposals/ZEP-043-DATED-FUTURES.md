# ZEP-043: Dated Futures Exchange

| Field       | Value                                          |
|-------------|------------------------------------------------|
| ZEP         | ZEP-043                                        |
| Title       | Dated Futures — Fixed-Expiry Oracle Settlement |
| Author      | Zebvix Core Team                               |
| Status      | IMPLEMENTED                                    |
| Category    | Trading / DeFi                                 |
| Created     | 2026-05-05                                     |
| Contracts   | ZbxDatedFutures.sol                            |
| Depends On  | ZbxOracle.sol / ZbxTwapOracle.sol              |
| Related     | ZEP-034 (Perpetuals — no-expiry version)       |

---

## Abstract

ZEP-043 introduces fixed-expiry futures contracts on ZBX Chain. Unlike perpetual futures (ZEP-034), dated futures have a defined settlement date. At expiry, the oracle price is locked and all open positions are cash-settled automatically. Multiple concurrent markets are supported (ZBX-JUN26, ZBX-DEC26, ETH-JUN26, etc.).

---

## Motivation

Dated futures serve different purposes than perpetuals:
- **Basis trading:** traders can exploit the spread between spot and futures price
- **Hedging:** known settlement date enables precise risk management
- **No funding rate:** position carry cost is expressed in the futures basis, not periodic payments
- **Institutional preference:** most traditional finance futures are dated contracts

---

## Specification

### Market Structure

Admin creates markets with:

```solidity
struct Market {
    string  name;             // "ZBX-JUN26"
    address oracle;           // price feed
    address collateralToken;  // margin (e.g. ZUSD)
    uint256 expiry;           // settlement timestamp
    uint256 maxLeverage;      // 1–50x (market-specific cap)
    uint256 settlementPrice;  // 0 until settled
    bool    settled;
    uint256 totalLongOI;
    uint256 totalShortOI;
}
```

### Position Lifecycle

```
openPosition(marketId, isLong, collateral, leverage)  ← before expiry
    ├─ closePosition(positionId)     ← before expiry, at mark price
    ├─ liquidate(positionId)         ← if equity < 4% of size
    └─ settleMarket(marketId)        ← anyone, after expiry → locks price
       └─ settlePosition(positionId) ← anyone, after market settled → pays out
```

### PnL Formula

```
pnl = (exitPrice − entryPrice) × size / entryPrice    [LONG]
    = (entryPrice − exitPrice) × size / entryPrice    [SHORT]

payout = collateral + pnl   (if pnl ≥ 0)
       = collateral − |pnl| (if pnl < 0, floored at 0)
```

### Leverage

- Open fee: 0.10% of collateral (deducted at open)
- Maintenance margin: 4.00% of position size
- Maximum leverage: configurable per market (global cap: 50×)

### Settlement

Anyone can trigger:
1. `settleMarket(marketId)` — reads oracle, locks `settlementPrice`
2. `settlePosition(positionId)` — calculates final PnL at locked price, pays trader

Settlement price priority:
1. `ISettlementOracle.getSettlementPrice(marketId)` — dedicated settlement feed
2. Fallback: `latestAnswer()` — spot oracle at time of settlement call

### Liquidation

Pre-expiry liquidation if:
```
equity = collateral + pnl < 4% × size
```

Liquidator bonus: **1.5% of collateral**. Remainder to protocol treasury.

### Example: ZBX-JUN26 Market

| Parameter | Value |
|-----------|-------|
| Name | ZBX-JUN26 |
| Oracle | ZbxTwapOracle |
| Collateral | ZUSD |
| Expiry | 2026-06-30 00:00 UTC |
| Max Leverage | 20× |
| Maintenance Margin | 4% |

Alice opens: 1000 ZUSD collateral, 5× leverage → 5000 ZUSD notional  
Entry price: 1.00 (normalised)  
Settlement price: 1.20 → PnL = +1000 ZUSD → Alice receives 2000 ZUSD

---

## Key Differences vs Perpetuals (ZEP-034)

| Feature | Perpetuals | Dated Futures |
|---------|-----------|---------------|
| Expiry | None | Fixed timestamp |
| Funding rate | Yes (hourly) | None |
| Settle mechanism | Close at mark price | Oracle lock at expiry |
| Max leverage | 20× | 50× (market cap) |
| Multiple markets | No (single oracle) | Yes (any number) |
| Basis | N/A | Spot − Futures spread |

---

## Security Considerations

| Risk | Mitigation |
|------|-----------|
| Oracle manipulation at settlement | Use TWAP oracle with 60min window |
| Settlement timing race | Price locked once by first `settleMarket()` caller |
| Keeper batch settlement gas | Keepers call `settlePosition()` per position; no batch function needed |
| Liquidation bonus griefing | 1.5% bounty — small for griefing, sufficient incentive |

---

## Implementation

- **Contract:** `zbx-chain-extracted/zbx-chain/contracts/ZbxDatedFutures.sol`
- **Key functions:** `createMarket(...)`, `openPosition(...)`, `closePosition(id)`, `settleMarket(id)`, `settlePosition(id)`, `liquidate(id)`, `unrealisedPnl(id)`, `isLiquidatable(id)`, `markPrice(marketId)`, `withdrawFees(token)`

---

## Status

IMPLEMENTED — Session 47 (2026-05-05). 0 audit findings.
