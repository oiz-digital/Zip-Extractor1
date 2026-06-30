# ZEP-039: Provably Fair On-Chain Raffle

| Field       | Value                           |
|-------------|---------------------------------|
| ZEP         | ZEP-039                         |
| Title       | Provably Fair On-Chain Raffle   |
| Author      | Zebvix Core Team                |
| Status      | IMPLEMENTED                     |
| Category    | Standard / Gaming               |
| Created     | 2026-05-05                      |
| Contracts   | ZbxRaffle.sol                   |
| Depends On  | ZbxVRF.sol                      |

---

## Abstract

ZEP-039 defines a trustless raffle contract where winner selection is provably random and cannot be manipulated by the raffle creator, participants, or validators. Uses ZbxVRF (commit-reveal + PREVRANDAO) for verifiable randomness.

---

## Motivation

Traditional online raffles rely on centralised operators. Even "on-chain" raffles that use `block.timestamp` or `blockhash` can be manipulated by validators. ZNS-031 VRF eliminates this — no party can know the winning ticket until after all commitments are locked.

---

## Specification

### Raffle Lifecycle

```
createRaffle() → buyTickets() × N → initiateDraw(seedHash) →
[next block] → completeDraw(seed) → winners paid out
```

### Prize Distribution

| Prize Tier | Share |
|-----------|-------|
| 1st place | 50% of pot |
| 2nd place | 30% of pot |
| 3rd place | 10% of pot |
| Raffle creator | 2.5% |
| Protocol treasury | 2.5% |

### Winner Selection

Using VRF randomness `R` and `n` total tickets:

```
winner[1st] = tickets[R % n]
winner[2nd] = tickets[(R / n) % n]
winner[3rd] = tickets[(R / n²) % n]
```

All three winners can coincide (same address wins multiple prizes) — this is accepted behaviour; a deduplication upgrade can be added in v2.

### Cancel & Refund

Creator can cancel a raffle before draw initiation. All ticket buyers receive full refund. Tickets are iterated and zeroed to prevent double-refund.

### Ticket Configuration

| Parameter | Description |
|-----------|-------------|
| `ticketToken` | ERC-20 or address(0) for native ZBX |
| `ticketPrice` | Cost per ticket |
| `maxTickets` | Cap (0 = unlimited, drawn by time) |
| `drawTime` | Earliest timestamp to initiate draw |

Minimum participants for draw: `MIN_DRAW_PARTICIPANTS = 3`

---

## Security Considerations

| Risk | Mitigation |
|------|-----------|
| Creator seeds manipulation | Commit-reveal: seed hash committed before reveal, 1+ block delay |
| Validator PREVRANDAO manipulation | Multi-block commit-reveal amortises this risk |
| Cancel double-refund | Ticket address zeroed after refund sent |
| Large raffle cancel gas | v1 acceptable (< 200 tickets); v2 should use pull-refund pattern |

---

## Implementation

- **Contract:** `zbx-chain-extracted/zbx-chain/contracts/ZbxRaffle.sol`
- **Key functions:** `createRaffle(...)`, `buyTickets(raffleId, count)`, `initiateDraw(raffleId, seedHash)`, `completeDraw(raffleId, seed)`, `cancelRaffle(raffleId)`, `ticketCount(raffleId)`, `ticketOwner(raffleId, ticketId)`

---

## Status

IMPLEMENTED — Session 46 (2026-05-05). 0 audit findings.
