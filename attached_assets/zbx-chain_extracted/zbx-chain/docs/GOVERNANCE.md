# ZBX Chain Governance

**Contracts**: ZbxGovernor.sol + ZbxTimelock.sol  
**Voting token**: ZBXGov (non-transferable, snapshot-based)  
**Quorum**: 4% of total supply  
**Voting delay**: 1 day  
**Voting period**: 5 days  
**Timelock**: 2 days

---

## Governable Parameters

| Parameter | Current | Min | Max |
|-----------|---------|-----|-----|
| Block gas limit | 30M | 10M | 60M |
| Validator max count | 100 | 4 | 1000 |
| Min validator stake | 100 ZBX | 10 | 1M |
| Bridge confirmation blocks | 12 | 6 | 50 |
| MEV redistribution: staker% | 30% | 0% | 100% |
| Oracle TWAP window | 30 min | 5 min | 24h |

---

## Proposal Lifecycle

```
Proposal submitted
     │ (1 day voting delay)
     ▼
Active voting
     │ (5 days)
     ▼
Quorum reached?
  ├─ NO  → Defeated
  └─ YES →
         Majority YES?
           ├─ NO  → Defeated
           └─ YES → Queued in timelock
                    │ (2 days)
                    ▼
                  Executed
```

---

## Creating a Proposal (CLI)

```bash
zbx governance propose \\
  --title "Increase block gas limit to 40M" \\
  --description "Network congestion justifies higher gas limit" \\
  --target 0xGovernor \\
  --calldata $(cast calldata "setBlockGasLimit(uint256)" 40000000)
```

---

## Voting

```bash
# Vote FOR a proposal
zbx governance vote --proposal-id 42 --support yes

# Vote AGAINST
zbx governance vote --proposal-id 42 --support no

# Vote with reason (on-chain record)
zbx governance vote --proposal-id 42 --support yes \\
  --reason "Increased throughput benefits all ecosystem participants"
```