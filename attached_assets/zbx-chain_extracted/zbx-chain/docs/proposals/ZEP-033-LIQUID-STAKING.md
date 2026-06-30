# ZEP-033: Liquid Staking (stZBX)

| Field       | Value                                  |
|-------------|----------------------------------------|
| ZEP         | ZEP-033                                |
| Title       | Liquid Staking — stZBX Receipt Token   |
| Author      | Zebvix Core Team                       |
| Status      | IMPLEMENTED                            |
| Category    | Standard / DeFi                        |
| Created     | 2026-05-05                             |
| Contracts   | ZbxLiquidStaking.sol                   |
| Depends On  | ZbxStaking.sol, Validator Reward Router |

---

## Abstract

ZEP-033 introduces liquid staking for ZBX Chain. Users deposit native ZBX and receive **stZBX** — a fully ERC-20-compatible, yield-bearing receipt token. stZBX can be used in DeFi (AMM liquidity, collateral, lending) while the underlying ZBX continues to earn validator rewards.

---

## Motivation

Traditional PoS staking locks capital. Validators earn block rewards but stakers sacrifice liquidity. Liquid staking solves this by issuing a tradeable token representing staked position + accrued yield, similar to Lido's stETH on Ethereum.

---

## Specification

### Token Model: Share-Based Accounting

```
Exchange Rate = totalPooled / totalSupply(stZBX)
```

When rewards are added to the pool:
- `totalPooled` increases
- `totalSupply` stays the same
- Each stZBX is redeemable for more ZBX → price appreciates

**Stake:**
```solidity
stZbxOut = zbxIn * totalSupply / totalPooled   // or 1:1 if pool is empty
```

**Unstake:**
```solidity
zbxOut = stZbxIn * totalPooled / totalSupply
```

### Example Flow

| Event | totalPooled | totalSupply | Rate |
|-------|-------------|-------------|------|
| Alice stakes 100 ZBX | 100 | 100 stZBX | 1.00 |
| Operator adds 10 ZBX rewards | 110 | 100 stZBX | 1.10 |
| Bob stakes 110 ZBX | 220 | 200 stZBX | 1.10 |
| Alice unstakes 100 stZBX | 110 | 100 stZBX | 1.10 — Alice receives 110 ZBX |

### Reward Injection

Only authorised operators (validator reward router) can call `addRewards()`. Operators are set by the contract owner (Zebvix DAO multisig).

### Security

- **CEI pattern:** shares burned before ZBX transferred on unstake
- **No withdrawal lock:** v1 has no unbonding period (slashing not yet implemented)
- **Operator whitelist:** only authorised addresses can inject rewards

### stZBX ERC-20 Functions

Full ERC-20 compliance: `transfer`, `transferFrom`, `approve`, `allowance`, `balanceOf`, `totalSupply`

---

## Implementation

- **Contract:** `zbx-chain-extracted/zbx-chain/contracts/ZbxLiquidStaking.sol`
- **Key functions:** `stake()`, `unstake(uint256)`, `addRewards()`, `exchangeRate()`, `previewStake()`, `previewUnstake()`

---

## Status

IMPLEMENTED — Session 46 (2026-05-05). 0 audit findings.
