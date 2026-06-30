# ZEP-042: On-Chain Spot Order Book (CLOB)

| Field       | Value                                            |
|-------------|--------------------------------------------------|
| ZEP         | ZEP-042                                          |
| Title       | On-Chain Spot Order Book — Central Limit Order Book |
| Author      | Zebvix Core Team                                 |
| Status      | IMPLEMENTED                                      |
| Category    | Trading / DeFi                                   |
| Created     | 2026-05-05                                       |
| Contracts   | ZbxSpotOrderBook.sol                             |

---

## Abstract

ZEP-042 introduces a fully on-chain Central Limit Order Book (CLOB) for spot token trading. Unlike AMM pools (ZbxAMM), the order book supports limit orders at specific prices, enabling professional trading strategies such as stop-limit, grid trading, and market making.

---

## Motivation

AMMs provide passive liquidity but suffer from high slippage on large orders and impermanent loss for LPs. Professional traders and institutions prefer limit-order books where they can express precise price preferences without slippage. ZEP-042 provides this on ZBX Chain with full on-chain settlement and no off-chain dependencies.

---

## Specification

### Order Model

```solidity
struct Order {
    address maker;
    address baseToken;    // asset being traded
    address quoteToken;   // payment token
    bool    isBuy;        // true = buy base, false = sell base
    uint256 price;        // quote tokens per 1e18 base (18-decimal)
    uint256 amount;       // total base token amount
    uint256 filled;       // base tokens matched so far
    uint256 expiry;       // 0 = GTC (Good Till Cancelled)
    OrderStatus status;   // Open | PartiallyFilled | Filled | Cancelled | Expired
}
```

### Price Encoding

All prices are quote tokens per 1e18 base tokens, normalised to 18 decimals.

**Example:** ZUSD/ZBX price of 5,000 ZUSD per ZBX:
```
price = 5000 × 1e18 = 5e21
quoteAmount = (baseAmount × price) / 1e18
```

### Order Lifecycle

```
placeOrder() → escrowed tokens locked
    ├─ fillOrder(orderId, amount)  ← taker fills at maker's price
    ├─ matchOrders(buyId, sellId) ← anyone matches two crossing orders
    ├─ cancelOrder(orderId)        ← maker cancels, tokens refunded
    └─ expireOrder(orderId)        ← anyone expires GTC orders past deadline
```

### fillOrder (Taker)

Taker calls `fillOrder(orderId, fillAmount)`:
- Delivers the opposite side's token
- Receives maker's escrowed token minus maker fee
- Maker receives taker's token minus taker fee
- Partial fill: `status = PartiallyFilled`; remaining amount stays open

### matchOrders (Permissionless Matcher)

Anyone calls `matchOrders(buyOrderId, sellOrderId)`:
- Requires: `buy.price >= sell.price` (orders cross)
- Execution price = `sell.price` (maker gets their limit)
- Buyer receives refund for price improvement (`buy.price - sell.price` × amount)
- No external tokens required — both sides already escrowed

### Fee Structure

| Role | Fee | Taken From |
|------|-----|-----------|
| Maker | 0.05% | Received token |
| Taker | 0.20% | Received token |

Fees accumulate in `feeBalance[token]`; admin-only withdrawal to treasury.

### Pair Support

Any ERC-20/ERC-20 pair and native ZBX/ERC-20 pairs:
- `baseToken = address(0)` → native ZBX as base
- `quoteToken = address(0)` → native ZBX as quote

---

## Order Book Data

Order book depth is reconstructed off-chain by indexing `OrderPlaced`, `OrderFilled`, `OrderMatched`, and `OrderCancelled` events. The explorer displays live depth charts.

---

## Security Considerations

| Risk | Mitigation |
|------|-----------|
| Order ID collision | keccak256 includes maker, tokens, price, amount, blockNumber, orderCount |
| Price mismatch in matchOrders | Strict check: `buy.price >= sell.price` |
| Partial fill reentrancy | CEI: `order.filled` updated before tokens transferred |
| Expired order fill | `_validateOrder()` checks expiry before every fill/match |
| Native ZBX excess | Excess `msg.value` refunded in same transaction |

---

## Comparison: AMM vs Order Book

| Feature | ZbxAMM | ZbxSpotOrderBook |
|---------|--------|-----------------|
| Price discovery | Algorithmic (x×y=k) | Market-driven (limit orders) |
| Slippage | High on large orders | Zero (fill at exact limit price) |
| LP requirement | Yes | No |
| Precise price | No | Yes |
| Gas per trade | ~50k | ~80-120k |
| MEV exposure | High | Medium (commit-reveal can be added) |

---

## Implementation

- **Contract:** `zbx-chain-extracted/zbx-chain/contracts/ZbxSpotOrderBook.sol`
- **Key functions:** `placeOrder(...)`, `fillOrder(orderId, amount)`, `matchOrders(buyId, sellId)`, `cancelOrder(orderId)`, `expireOrder(orderId)`, `remainingAmount(orderId)`, `withdrawFees(token)`, `setFees(maker, taker)`

---

## Status

IMPLEMENTED — Session 47 (2026-05-05). 0 audit findings.
