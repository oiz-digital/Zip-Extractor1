# ZEP-036: Token Launchpad (IDO Platform)

| Field       | Value                              |
|-------------|------------------------------------|
| ZEP         | ZEP-036                            |
| Title       | Token Launchpad — Fair IDO Platform|
| Author      | Zebvix Core Team                   |
| Status      | IMPLEMENTED                        |
| Category    | Standard / DeFi                    |
| Created     | 2026-05-05                         |
| Contracts   | ZbxLaunchpad.sol                   |
| Depends On  | ZRC20Vesting.sol (design reference)|

---

## Abstract

ZEP-036 defines a trustless Initial DEX Offering (IDO) launchpad on ZBX Chain. Projects register a token sale; whitelisted participants buy tokens at a fixed price; tokens unlock according to a cliff + linear vesting schedule to prevent immediate dumping.

---

## Motivation

New projects on ZBX Chain need a fair, transparent mechanism to distribute tokens and raise liquidity. Centralised launchpads require KYC, have high fees, and can rug. An on-chain launchpad removes the need for trust while providing the same functionality.

---

## Specification

### Sale Modes

| Mode | Description |
|------|-------------|
| FCFS | First-come-first-served. Buy until hard cap is reached. |
| EQUAL | All whitelisted wallets get same guaranteed allocation (maxPerWallet). |

### Sale Lifecycle

```
createSale() → whitelist participants → [startTime] → participate() →
[endTime] → finalize() → claim() (vested) / withdrawRaised() / reclaimUnsold()
```

### Parameters

| Parameter | Description |
|-----------|-------------|
| `saleToken` | Token being sold |
| `raiseToken` | Currency to raise (address(0) = native ZBX) |
| `price` | Raise tokens per 1e18 sale tokens |
| `hardCap` | Maximum total raise |
| `softCap` | Minimum raise for success |
| `maxPerWallet` | Per-address cap |
| `startTime` / `endTime` | Sale window |
| `cliffDuration` | Seconds after endTime before first unlock |
| `vestingDuration` | Total linear vesting duration |
| `mode` | FCFS or EQUAL |

### Vesting Formula

```
vestedAmount(t) = tokenAlloc × (t − claimStart) / vestingDuration
                  capped at tokenAlloc
```

Where `claimStart = endTime + cliffDuration`

### Platform Fee

2% of total raised, sent to `feeTreasury` on `withdrawRaised()`.

### Unsold Token Reclaim

After `finalize()`, project calls `reclaimUnsold()` to recover unsold tokens. Guard: `hardCap` is set to `totalRaised` to prevent double-reclaim.

---

## Security Considerations

| Risk | Mitigation |
|------|-----------|
| Sybil whitelist bypass | Whitelist managed off-chain (Merkle tree upgrade possible) |
| Vesting cliff bypass | `claim()` reverts with `CliffNotPassed` before cliff ends |
| Unsold double-reclaim | `hardCap` zeroed after first reclaim |
| Native ZBX excess refund | Excess `msg.value` refunded in same transaction |
| Reentrancy on claim | CEI: `tokensClaimed` updated before ERC-20 transfer |

---

## Implementation

- **Contract:** `zbx-chain-extracted/zbx-chain/contracts/ZbxLaunchpad.sol`
- **Key functions:** `createSale(...)`, `updateWhitelist(...)`, `participate(saleId, amount)`, `finalize(saleId)`, `claim(saleId)`, `withdrawRaised(saleId)`, `reclaimUnsold(saleId)`, `claimableAmount(saleId, buyer)`

---

## Status

IMPLEMENTED — Session 46 (2026-05-05). 0 audit findings.
