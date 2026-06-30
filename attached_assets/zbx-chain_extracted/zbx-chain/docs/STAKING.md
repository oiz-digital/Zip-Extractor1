# ZBX Chain Staking Guide

**Minimum validator stake**: 100 ZBX  
**Minimum delegator stake**: 10 ZBX  
**Unbonding period**: 7 days  
**APR**: 12–18% (varies with participation rate)

---

## Overview

ZBX Chain uses Delegated Proof of Stake (DPoS). Two roles:

| Role | Requirement | Reward |
|---|---|---|
| **Validator** | Run a node + stake ≥ 100 ZBX | Block rewards + tx fees + MEV share |
| **Delegator** | Stake ≥ 10 ZBX to a validator | % of validator rewards |

> **Note**: Low minimums are intentional — anyone can participate. Security comes from HotStuff-BFT's 2/3 quorum requirement, not from large individual stakes.

---

## Staking ZBX (Delegator)

### Via CLI
```bash
zbx stake delegate \
  --validator 0xABCD...  \
  --amount 10            \  # minimum 10 ZBX
  --from mywallet
```

### Via Smart Contract
```solidity
ZbxStaking staking = ZbxStaking(STAKING_ADDRESS);
staking.delegate{value: 10 ether}(validatorAddress); // min 10 ZBX
```

---

## Running a Validator

### Requirements
- **Hardware**: 16 core CPU, 64 GB RAM, 2 TB NVMe SSD, 1 Gbps uptime
- **Stake**: 100 ZBX self-stake minimum (+ additional from delegators)
- **Uptime**: ≥ 95% required (slashed below)

### Setup

#### 1. Generate Validator Keys

Use the included `zbx-keygen` binary (built alongside `zbx-node`):

```bash
# Generate 1 keypair — follow instructions in the output
./zbx-keygen --count 1 --output text

# For testnet launch with 3 validators:
./zbx-keygen --count 3 --output text
```

Output includes:
- **EVM Address** — add to `testnet-genesis.json` validators[]
- **BLS PubKey** — share with other validators for their TOML `[[chain.extra_validators]]`
- **BLS PrivKey** — set as `VALIDATOR_KEY` env var on your VPS (keep secret)
- Ready-to-paste genesis JSON and TOML snippets

#### 2. Configure and Start

```bash
# Set private key (never put in config file)
export VALIDATOR_KEY=0x<bls_privkey>

# Start node
zbx-node \
  --network testnet \
  --config /etc/zbx/testnet.toml \
  --validator \
  --log-level info
```

#### 3. Register (on-chain staking)

```bash
zbx-cli staking register \
  --amount 100 \
  --commission 5 \
  --rpc http://localhost:18545
```

---

## Rewards

Rewards are distributed each **epoch** (every 43,200 blocks ≈ 2.5 days at 5s blocks):

```
Block reward formula:
  base_reward = 3 ZBX / block  (Era 0; halves every 25M blocks)
  validator_share = base_reward × (1 - delegator_ratio) × blocks_produced
  delegator_share = base_reward × delegator_ratio (split proportionally)
```

**MEV rewards** (from zbx-mev redistribution): additional ~0.5–2% APR

---

## Slashing

| Infraction | Penalty |
|---|---|
| Downtime (missed >5% blocks in epoch) | 0.01% of stake |
| Double signing (equivocation) | 5% of stake |
| Invalid state proof | 10% of stake |

Slashed funds: 50% burned, 50% to reporter's reward.

---

## Unbonding

```bash
zbx stake undelegate --amount 500 --validator 0xABCD...
```

Funds are locked for **7 days** (security against long-range attacks).  
During unbonding, funds earn **no rewards** and are **not slashable**.

### Partial Unbonding — Chunk Tracking

Each `undelegate` call creates an **`UnbondingChunk`** recorded on the
`DelegationRecord`.  Multiple partial undelegations accumulate independently:

```
DelegationRecord {
    amount:           1500 ZBX   // remaining active stake
    unbonding_chunks: [
        { amount: 500 ZBX, unlock_at: epoch 142 },  // first partial
        { amount: 300 ZBX, unlock_at: epoch 149 },  // second partial
    ]
}
```

`withdraw_delegation()` drains **all matured chunks** (where `current_epoch >=
unlock_at`) in a single call.  Immature chunks stay on the record until their
unlock epoch.  A full undelegate sets `status = Unbonding` and pushes the
entire remaining amount as a final chunk; once withdrawn the record is removed.

> **Implementation note (Session 19 — 2026-05-03):** Prior to this fix,
> partial undelegations silently reduced `amount` with no chunk tracking —
> funds became permanently inaccessible (fund-loss vector NEW-HIGH-02).
> The chunk model closes this gap.

---

## Staking Constants (Code)

Defined in `crates/zbx-contracts/src/staking_escrow.rs`:

| Constant | Value |
|---|---|
| `MIN_STAKE` | 100 ZBX (= `100 * 10^18` Wei) |
| `MIN_DELEGATION` | 10 ZBX (= `10 * 10^18` Wei) |
| Max commission | 20% (2000 bps) |
| Slash %: downtime | 0.01% |
| Slash %: double-sign | 5% |
