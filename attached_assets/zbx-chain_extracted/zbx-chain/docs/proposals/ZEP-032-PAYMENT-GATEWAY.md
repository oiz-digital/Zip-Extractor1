# ZEP-032: Crypto Payment Gateway

| Field       | Value                                         |
|-------------|-----------------------------------------------|
| ZEP         | ZEP-032                                       |
| Title       | Crypto Payment Gateway                        |
| Author      | Zebvix Core Team                              |
| Status      | IMPLEMENTED                                   |
| Category    | Standard / DeFi                               |
| Created     | 2026-05-05                                    |
| Updated     | 2026-05-05                                    |
| Contracts   | ZbxPaymentGateway.sol                         |
| Rust Crate  | zbx-payment                                   |

---

## Abstract

ZEP-032 defines a trustless crypto payment gateway on ZBX Chain. Merchants register on-chain, create invoices, and receive payments in any supported token — optionally auto-swapped into their preferred settlement currency via ZbxRouter.

---

## Motivation

E-commerce and SaaS businesses need a permissionless payment rail on ZBX Chain. Existing solutions require centralised operators. ZEP-032 provides a fully on-chain, non-custodial payment gateway that integrates natively with the ZBX DEX for automatic currency conversion.

---

## Specification

### Merchant Registration

```solidity
function registerMerchant(address payoutAddress, string calldata name)
    external returns (bytes32 merchantId)
```

- `merchantId = keccak256(msg.sender ‖ name ‖ block.number)`
- Merchants can update their `payoutAddress` at any time
- Merchant list is public and queryable

### Invoice Lifecycle

```
OPEN → PAID → REFUNDED
     ↘ EXPIRED
```

| State | Condition |
|-------|-----------|
| OPEN | Created, awaiting payment |
| PAID | `totalPaid >= amount` |
| EXPIRED | `block.timestamp > deadline` and not fully paid |
| REFUNDED | Merchant issued refund within 48h window |

### Payment Methods

**Direct payment (`pay`):**
```solidity
function pay(bytes32 invoiceId, address token, uint256 amount) external
```
Customer pays in any accepted token. Partial payments accumulate until invoice is fully paid.

**Auto-swap payment (`payWithConvert`):**
```solidity
function payWithConvert(bytes32 invoiceId, address fromToken, uint256 maxIn) external
```
Customer's token is automatically swapped via `ZbxRouter` to the invoice token. Excess is refunded.

### Protocol Fee

- Default: 0.5% of payment amount
- Maximum: 2%
- Recipient: treasury address (governance-controlled)

### Refund Window

- 48 hours after `PAID` status
- Merchant calls `refund(invoiceId)` — full amount returned to payer
- Merchant's withdrawal balance is debited

### Webhook Events

All state transitions emit structured events for off-chain indexing:

| Event | Fields |
|-------|--------|
| `MerchantRegistered` | merchantId, payoutAddress, name |
| `InvoiceCreated` | invoiceId, merchantId, orderId, token, amount, deadline |
| `PaymentReceived` | invoiceId, payer, token, amount, totalPaid |
| `InvoicePaid` | invoiceId, merchantId, totalAmount |
| `Refunded` | invoiceId, payer, amount |
| `Withdrawn` | merchantId, recipient, amount |

---

### Rust Crate: zbx-payment

| Module | Purpose |
|--------|---------|
| `merchant.rs` | Merchant registry cache, lifetime volume analytics |
| `invoice.rs` | Invoice store with order-ID index, partial-payment tracking |
| `webhook.rs` | Typed webhook payload schemas (HMAC-signed JSON) |
| `converter.rs` | Oracle-backed price estimation for UX |

---

## Security Considerations

| Risk | Mitigation |
|------|-----------|
| Double-payment | `totalPaid` tracked cumulatively; excess auto-refunded |
| Swap slippage | `maxIn` parameter caps maximum customer spend |
| Reentrancy in pay() | CEI: invoice state updated before token transfer |
| Fee manipulation | Protocol fee capped at 2%, governance-controlled |
| Refund window abuse | 48h window enforced by `block.timestamp` check |

---

## Implementation

- **Contract:** `zbx-chain-extracted/zbx-chain/contracts/ZbxPaymentGateway.sol`
- **Crate:** `zbx-chain-extracted/zbx-chain/crates/zbx-payment/`
- **Build:** 0 errors

---

## Status

IMPLEMENTED — Session 45 (2026-05-05). 0 audit findings.
