# ZEP-041: On-Chain Card Game Engine

| Field       | Value                              |
|-------------|------------------------------------|
| ZEP         | ZEP-041                            |
| Title       | On-Chain Card Game Engine          |
| Author      | Zebvix Core Team                   |
| Status      | IMPLEMENTED                        |
| Category    | Standard / Gaming                  |
| Created     | 2026-05-05                         |
| Contracts   | ZbxCardGame.sol                    |
| Depends On  | ZbxVRF.sol (design reference)      |

---

## Abstract

ZEP-041 defines a trustless on-chain card game engine. A standard 52-card deck is shuffled using a multi-party commit-reveal VRF scheme so that no single player (including the dealer) can predict or control the shuffle outcome. Game state (hands, deck) is stored fully on-chain.

---

## Motivation

Traditional card games require a trusted shuffle server. Blockchain card games often use weak randomness (block hash) exploitable by miners. ZEP-041 combines commit-reveal with `block.prevrandao` so any attempt to manipulate the shuffle requires control over all participating seeds AND the block randomness simultaneously.

---

## Specification

### Card Encoding

```
cardIndex = rank × 4 + suit
rank: 0–12 (displayed as 2–14, where 14 = Ace)
suit: 0=Clubs, 1=Diamonds, 2=Hearts, 3=Spades

decodeCard(cardIndex) → (rank 2-14, suit 0-3)
```

### Room Lifecycle

```
createRoom(stake, maxPlayers) → roomId
joinRoom(roomId) × (maxPlayers-1)      ← auto-enters Committing phase
commitSeed(roomId, keccak256(seed))    ← each player (parallel)
revealSeed(roomId, seed)               ← each player
                                       ← deck auto-shuffled when all reveal
dealCards(roomId)                      ← 5 cards per player
determineWinner(roomId)               ← highest single card wins; payout
```

### Shuffle Algorithm

When all seeds are revealed:

1. `combined = XOR(seed[0], seed[1], ..., seed[n-1])`
2. `randomness = keccak256(block.prevrandao ‖ combined ‖ roomId)`
3. Fisher-Yates shuffle with iterative re-hash:

```
for i = 51 downto 1:
    j = randomness % (i + 1)
    swap(deck[i], deck[j])
    randomness = keccak256(randomness)
```

### Security Properties

- **No single-player control:** Final randomness depends on ALL seeds XORed together — any one honest player makes the shuffle unpredictable
- **No dealer advantage:** Dealer's seed is just one input; changing it changes the entire shuffle unpredictably
- **Validator-resistance:** `block.prevrandao` inclusion makes validator manipulation expensive

### Payout

| Recipient | Share |
|-----------|-------|
| Winner | 98% of total pot |
| Protocol treasury | 2% |

### Current Win Condition

`determineWinner()` awards the pot to the player with the highest single card by rank (2 < 3 < ... < King < Ace).

**Upgrade path (ZEP-042):** Full 5-card poker hand evaluator (straight, flush, full house, etc.) can be added as a standalone library imported by `ZbxCardGame`.

---

## Room Parameters

| Parameter | Range | Default |
|-----------|-------|---------|
| `maxPlayers` | 2–8 | — |
| `buyIn` | > 0 | — |
| `stakeToken` | any ERC-20 or ZBX | — |
| `HAND_SIZE` | constant | 5 |
| `DECK_SIZE` | constant | 52 |

---

## Security Considerations

| Risk | Mitigation |
|------|-----------|
| Reveal withholding | Dealer adds reveal timeout; non-revealer forfeits in v2 |
| Seed bruteforce | Seeds are `bytes32` (256-bit) — computationally infeasible |
| Front-running commitSeed | Commitment phase: only hash revealed; actual seed hidden |
| Reentrancy in payout | `state = Over` set before `_sendStake()` |

---

## Implementation

- **Contract:** `zbx-chain-extracted/zbx-chain/contracts/ZbxCardGame.sol`
- **Key functions:** `createRoom(...)`, `joinRoom(roomId)`, `commitSeed(roomId, hash)`, `revealSeed(roomId, seed)`, `dealCards(roomId)`, `determineWinner(roomId)`, `getHand(roomId, player)`, `decodeCard(cardIndex)`

---

## Status

IMPLEMENTED — Session 46 (2026-05-05). 0 audit findings.
