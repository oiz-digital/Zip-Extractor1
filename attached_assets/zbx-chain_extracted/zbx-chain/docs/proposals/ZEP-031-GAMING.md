# ZEP-031: On-Chain Gaming Framework

| Field       | Value                                      |
|-------------|--------------------------------------------|
| ZEP         | ZEP-031                                    |
| Title       | On-Chain Gaming Framework                  |
| Author      | Zebvix Core Team                           |
| Status      | IMPLEMENTED                                |
| Category    | Standard / Gaming                          |
| Created     | 2026-05-05                                 |
| Updated     | 2026-05-05                                 |
| Contracts   | ZbxVRF.sol, ZbxGameEscrow.sol, ZbxGameItems.sol |
| Rust Crate  | zbx-gaming                                 |

---

## Abstract

ZEP-031 introduces a native on-chain gaming primitive layer for ZBX Chain. Three smart contracts and a supporting Rust crate provide verifiable randomness, trustless prize escrow, and ERC-1155 game item NFTs — enabling fully on-chain games that no single party can manipulate.

---

## Motivation

Existing gaming on EVM chains relies on centralised RNG (random number generators) which can be manipulated by miners/validators. Players must trust the game operator for fund custody. ZBX Chain should provide trustless, verifiable building blocks so any developer can create provably-fair games without re-implementing security-critical primitives.

---

## Specification

### ZbxVRF.sol — On-Chain Verifiable Randomness

**Mechanism:** Commit-reveal scheme using `block.prevrandao` as entropy source.

```
Phase 1 (Commit):   requestRandom(keccak256(seed)) → requestId
Phase 2 (Reveal):   fulfillRandom(requestId, seed) → uint256 randomness
```

**Security invariants:**
- `MIN_REVEAL_DELAY = 1` block — prevents same-block manipulation
- `MAX_REVEAL_DELAY = 256` blocks — prevents stale-blockhash attacks
- `requestId = keccak256(requester ‖ seedHash ‖ blockNumber)`
- `randomness = keccak256(block.prevrandao ‖ seed ‖ requestId)`

**Errors:** `RevealTooEarly`, `RevealTooLate`, `InvalidSeed`, `AlreadyFulfilled`

---

### ZbxGameEscrow.sol — Trustless Prize Custody

**Flow:**
1. Player A calls `createMatch(stakeAmount, token, gameContract)` → `matchId`
2. Player B calls `joinMatch(matchId)` — both stakes locked
3. `gameContract` (authorised resolver) calls `resolveMatch(matchId, winner)`
4. Winner receives both stakes minus protocol fee (1%)

**Tournament support:** `createTournament(players[], stakes[])` → pot split among top-N

**Errors:** `MatchFull`, `MatchNotOpen`, `NotResolver`, `MatchExpired`

---

### ZbxGameItems.sol — ERC-1155 Game Items

- Standard ERC-1155 multi-token (fungible + non-fungible hybrid)
- **Soulbound:** per-token `soulbound` flag — transfer blocked
- **On-chain attributes:** `damage`, `defense`, `rarity`, `level` stored in contract
- **EIP-2981 royalties:** configurable per-token royalty (max 10%)
- **Batch operations:** `mintBatch`, `burnBatch`, `safeBatchTransferFrom`

---

### Rust Crate: zbx-gaming

| Module | Purpose |
|--------|---------|
| `vrf.rs` | VRF request lifecycle management, seed verification |
| `session.rs` | Game session index — active matches, tournament brackets |
| `leaderboard.rs` | Top-N leaderboard with proportional reward distribution |
| `items.rs` | Item state cache — attribute indexing, rarity distribution |

---

## Security Considerations

| Risk | Mitigation |
|------|-----------|
| Validator manipulates `PREVRANDAO` | Cost exceeds benefit for typical game stakes; Randao is 51%-attack resistant |
| Player withholds reveal | `MAX_REVEAL_DELAY=256` — after which anyone can cancel and claim forfeit |
| Reentrancy in escrow payout | CEI pattern: match state set to RESOLVED before token transfer |
| Fake game contract resolves | `resolveMatch` only callable by whitelisted `gameContract` set at match creation |

---

## Implementation

- **Contracts:** `zbx-chain-extracted/zbx-chain/contracts/ZbxVRF.sol`, `ZbxGameEscrow.sol`, `ZbxGameItems.sol`
- **Crate:** `zbx-chain-extracted/zbx-chain/crates/zbx-gaming/`
- **Build:** `cargo check` — 0 errors

---

## Status

IMPLEMENTED — Session 45 (2026-05-05). 0 audit findings.
