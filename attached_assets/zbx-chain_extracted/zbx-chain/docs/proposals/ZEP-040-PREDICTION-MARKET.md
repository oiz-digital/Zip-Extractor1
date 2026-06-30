# ZEP-040: On-Chain Prediction Market

| Field       | Value                             |
|-------------|-----------------------------------|
| ZEP         | ZEP-040                           |
| Title       | On-Chain Prediction Market        |
| Author      | Zebvix Core Team                  |
| Status      | IMPLEMENTED                       |
| Category    | Standard / Gaming / DeFi          |
| Created     | 2026-05-05                        |
| Contracts   | ZbxPredictionMarket.sol           |

---

## Abstract

ZEP-040 defines a trustless binary prediction market. Anyone creates a YES/NO question with a resolution deadline and a resolver address. Bettors stake tokens on their chosen outcome. After deadline the resolver settles the market; winners share the pot proportionally to their stake.

---

## Motivation

Prediction markets are powerful tools for price discovery and risk hedging. On-chain prediction markets eliminate custodial risk, ensure transparent payout rules, and allow anyone to create a market on any verifiable event.

---

## Specification

### Market Lifecycle

```
createMarket() → bet(YES/NO) × N → [deadline] → resolve(outcome) →
claim() (winners) + withdrawCreatorFee()
```

### Outcomes

| Outcome | Result |
|---------|--------|
| `Yes` | YES bettor pool shares the total pot |
| `No` | NO bettor pool shares the total pot |
| `Void` | All bettors receive full refund |

### Payout Formula

For outcome `Yes`:

```
payout(user) = (yesBet[user] / totalYesPool) × netPot

netPot = totalPot × (1 − protocolFee − creatorFee)
       = totalPot × 0.97
```

### Fee Structure

| Fee | Rate | Recipient |
|-----|------|-----------|
| Protocol | 2% | Treasury |
| Creator | 1% | Market creator |

### Resolver

The `resolver` address is set at market creation. It can be:
- A Chainlink-style oracle adapter
- A DAO multisig
- ZbxOracle (for price-event markets)
- Any trusted address

Resolver can only call `resolve()` after `deadline` passes.

### Odds Display

```solidity
getOdds(marketId) → (yesOdds, noOdds)   // in basis points
// e.g., (6500, 3500) = 65% YES / 35% NO implied probability
```

Starting odds: 50/50 when no bets placed.

### Estimate Payout (Pre-Resolution)

```solidity
estimatePayout(marketId, bettor) → uint256
```

Shows expected winnings if the bettor's current side wins — useful for UI.

---

## Use Cases

| Event | Resolver |
|-------|---------|
| "ZBX > $1 by 2027?" | ZbxTwapOracle |
| "Team A wins World Cup?" | Zebvix DAO multisig |
| "Proposal #42 passes?" | ZbxGovernor (on-chain) |
| "ZBX TVL > $100M?" | ZbxTvlOracle |

---

## Security Considerations

| Risk | Mitigation |
|------|-----------|
| Resolver manipulation | DAO multisig resolver + community watchdog |
| Whale odds manipulation | Fully parimutuel — larger pool = better odds for opponents |
| Fee double-take | Fees deducted from `netPot` at claim time; creator/protocol withdraw separately |
| Void payout correctness | VOID path returns `yesBet + noBet` — no fee taken |

---

## Implementation

- **Contract:** `zbx-chain-extracted/zbx-chain/contracts/ZbxPredictionMarket.sol`
- **Key functions:** `createMarket(...)`, `bet(marketId, isYes, amount)`, `resolve(marketId, outcome)`, `claim(marketId)`, `withdrawCreatorFee(marketId)`, `estimatePayout(...)`, `getOdds(...)`

---

## Status

IMPLEMENTED — Session 46 (2026-05-05). 0 audit findings.
