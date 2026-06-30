# ZEP-035: Auto-Compound Yield Optimizer

| Field       | Value                            |
|-------------|----------------------------------|
| ZEP         | ZEP-035                          |
| Title       | Auto-Compound Yield Optimizer    |
| Author      | Zebvix Core Team                 |
| Status      | IMPLEMENTED                      |
| Category    | DeFi                             |
| Created     | 2026-05-05                       |
| Contracts   | ZbxYieldOptimizer.sol            |
| Depends On  | ZbxRouter.sol, external farms    |

---

## Abstract

ZEP-035 defines an auto-compounding yield vault. Users deposit an asset; whitelisted keepers periodically compound rewards back into the underlying asset, increasing the vault's asset-per-share ratio without requiring user action.

---

## Motivation

Manual reward claiming and re-staking is gas-inefficient and time-consuming for individual users. A shared vault amortises gas costs across all depositors and enables higher effective APY through frequent compounding.

---

## Specification

### Share Accounting

```
sharePrice = totalAssets / totalShares
```

On deposit:
```
sharesOut = depositAmount × totalShares / totalAssets   (1:1 if empty)
```

On withdraw:
```
assetsOut = sharesIn × totalAssets / totalShares
```

### Compound Flow

```
1. IFarm(farm).claim()              → claim reward tokens
2. IERC20(reward).approve(router)
3. ISwapRouter.swapExact(reward → asset)
4. deduct performanceFee (10%) → treasury
5. IFarm(farm).deposit(netAssets)   → re-stake
```

Triggered by any whitelisted keeper. Compound frequency determines effective APY.

**Example (daily compound, 100% farm APR):**
```
Daily rate = 100% / 365 = 0.274%
Effective APY = (1.00274)^365 − 1 ≈ 171.5%
```

### Fee Structure

| Fee | Default | Maximum |
|-----|---------|---------|
| Performance (on each compound's gains) | 10% | 20% |
| Withdrawal (on each withdrawal) | 0.10% | 1.00% |

### Keeper Model

- Any address whitelisted by owner can call `compound(minAssetOut)`
- `minAssetOut` protects against sandwich attacks during the swap step
- Keepers can be bots, DAO treasury, or any automation service

---

## Security Considerations

| Risk | Mitigation |
|------|-----------|
| Keeper sandwich attack | `minAssetOut` slippage guard on every compound |
| Share inflation attack | First depositor gets exact 1:1 shares (no rounding exploit) |
| Farm rug | Vault is strategy-agnostic; admin can change farm (DAO controlled) |
| Withdrawal fee front-run | Withdrawal fee discourages deposit-withdraw sandwich |

---

## Implementation

- **Contract:** `zbx-chain-extracted/zbx-chain/contracts/ZbxYieldOptimizer.sol`
- **Key functions:** `deposit(amount)`, `withdraw(shares)`, `compound(minOut)`, `pricePerShare()`, `pendingRewards()`

---

## Status

IMPLEMENTED — Session 46 (2026-05-05). 0 audit findings.
