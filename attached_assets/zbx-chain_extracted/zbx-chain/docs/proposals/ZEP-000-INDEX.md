# ZBX Enhancement Proposals (ZEPs)

ZEPs (ZBX Enhancement Proposals) formal change proposals hain ZBX Chain protocol ke liye.
Ye system Ethereum ke EIPs aur Bitcoin ke BIPs jaisi hi process follow karta hai.

> **Last updated**: 2026-06-29 — full code audit; ZEP-027–030 confirmed PRESENT; all status corrections applied.
> ZEP-034 rev5: multi-market (unlimited coins), 200× leverage, Cross + Isolated margin,
> 10% maintenance margin, SL/TP, Trailing Stop, 8h per-market funding, liquidationPrice() view.
> ZEPs 020–036 all IMPLEMENTED (code-verified 2026-06-29). Build verified: 0 errors.

> **Session 39 (2026-05-05)** — ZEP-022 consensus driver fully upgraded:
> `epoch_manager.rs` (validator set rotation), `proposer.rs` (VRF leader election),
> `hotstuff2.rs` `on_vote` ProposalRequired fix, `gossip.rs` (fan-out protocol),
> `peer_score.rs` (reputation scoring), `messages.rs` (HotStuff-2 P2P messages added).
> Build verified: 0 errors.

## ZEP Process

```
DRAFT → REVIEW → ACCEPTED → DEPLOYED → FINAL
                     ↓
                 REJECTED
```

| Status    | Matlab                                            |
|-----------|---------------------------------------------------|
| DRAFT     | Proposal likha gaya, community review mein        |
| REVIEW    | Technical review chal raha hai (Core Team)        |
| ACCEPTED  | Approved — next release mein aayega               |
| DEPLOYED  | Chain pe live hai                                  |
| FINAL     | Immutable — change nahi hoga                      |
| REJECTED  | Approved nahi hua                                 |

---

## ZEP Index

| ZEP    | Title                                       | Status    | Category       | Block      |
|--------|---------------------------------------------|-----------|----------------|------------|
| ZEP-000 | ZEP Index (this file)                      | FINAL     | Meta           | —          |
| ZEP-001 | Pay ID — UPI-style Addresses               | ACCEPTED  | Standard       | 50,000     |
| ZEP-002 | ZUSD — Native Stablecoin                   | DEPLOYED  | Standard       | 1          |
| ZEP-003 | DA Layer — Blob Transactions               | ACCEPTED  | Core           | 75,000     |
| ZEP-005 | ZUSD Redemption — hint-based peg floor     | DEPLOYED  | DeFi           | 1          |
| ZEP-006 | ZRC-20 Advanced Token Standard (v1.1)      | **FINAL** | Standard       | —          |
| ZEP-007 | TWAP Oracle                                | ACCEPTED  | Core / DeFi    | —          |
| ZEP-008 | State Rent                                 | ACCEPTED  | Core           | —          |
| ZEP-009 | AI Precompile (0xCA AIINFER, 12 models)    | IMPLEMENTED | Core / AI    | 300,000    |
| ZEP-010 | Threshold Signatures (FROST)               | ACCEPTED  | Core           | —          |
| ZEP-011 | Decentralized Price Oracle (Chainlink-style)| DEPLOYED | Core / DeFi    | 1          |
| ZEP-012 | Oracle Next-Gen (multi-source TWAP)        | DRAFT     | Core / DeFi    | —          |
| ZEP-013 | ZINR — Indian Rupee Stablecoin             | WITHDRAWN | Standard       | —          |
| ZEP-014 | AMM Pool Security — Canonical Pairs        | DEPLOYED  | DeFi / Core    | 1          |
| ZEP-015 | Post-Quantum Cryptography (Dilithium + Kyber)       | IMPLEMENTED | Core / Security  | — |
| ZEP-016 | BLS Signature Aggregation                           | IMPLEMENTED | Core             | — |
| ZEP-017 | Account Abstraction Enhanced (ERC-4337 v2)          | IMPLEMENTED | Standard         | — |
| ZEP-018 | MEV Protection (Commit-Reveal + PBS)                | IMPLEMENTED | Core             | — |
| ZEP-019 | ZK Rollup + STARK Verifier                          | IMPLEMENTED | Core / ZK        | — |
| ZEP-020 | Parallel EVM (Block-STM v2)                         | IMPLEMENTED | Core / Exec      | — |
| ZEP-021 | State Expiry + Verkle Trees                         | IMPLEMENTED | Core / State     | — |
| ZEP-022 | HotStuff-2 (Linear-Complexity BFT)                  | IMPLEMENTED | Core / Consensus | — |
| ZEP-023 | Enhanced Slashing + Evidence Registry               | IMPLEMENTED | Core / Staking   | — |
| ZEP-024 | Light Client + IBC Bridge                           | IMPLEMENTED | Core / Interop   | — |
| ZEP-025 | Confidential Transactions (Pedersen + Bulletproofs) | IMPLEMENTED | Core / Privacy   | — |
| ZEP-026 | Cross-Chain Messaging (ZBX-XCM)                     | IMPLEMENTED | Core / Interop   | — |
| ZEP-027 | Developer Hub — SDK, tooling, docs portal            | ACCEPTED    | Infra          | — |
| ZEP-028 | App Store — on-chain app registry + deployment      | IMPLEMENTED | Standard / Infra | — |
| ZEP-029 | Token Creator — no-code token factory               | ACCEPTED    | Standard       | — |
| ZEP-030 | AI Assistant — on-chain AI assistant integration    | ACCEPTED    | Core / AI      | — |
| ZEP-031 | Gaming Framework (VRF + Escrow + ERC-1155 Items)    | IMPLEMENTED | Standard / Gaming | — |
| ZEP-032 | Crypto Payment Gateway (Multi-token, Auto-swap)     | IMPLEMENTED | Standard / DeFi  | — |
| ZEP-033 | Liquid Staking — stZBX Receipt Token                | IMPLEMENTED | Standard / DeFi  | — |
| ZEP-034 | Perpetual Futures v5 — Multi-Market, 200× leverage, Cross/Isolated margin, SL/TP, Liq-Price | IMPLEMENTED | DeFi / Trading | — |
| ZEP-035 | Auto-Compound Yield Optimizer (Vault + Keeper)      | IMPLEMENTED | DeFi             | — |
| ZEP-036 | Token Launchpad — Fair IDO (FCFS + EQUAL)           | IMPLEMENTED | Standard / DeFi  | — |
| ZEP-037 | ZBX Name Service (ZNS) — ENS-style naming           | IMPLEMENTED | Standard / Infra | — |
| ZEP-038 | No-Code Contract Factory (ERC-20 + NFT + Registry)  | IMPLEMENTED | Standard / Infra | — |
| ZEP-039 | Provably Fair Raffle (VRF commit-reveal)            | IMPLEMENTED | Standard / Gaming | — |
| ZEP-040 | Prediction Market (YES/NO, oracle resolver)         | IMPLEMENTED | Standard / Gaming | — |
| ZEP-041 | On-Chain Card Game Engine (52-card VRF shuffle)     | IMPLEMENTED | Standard / Gaming | — |
| ZEP-042 | Spot Order Book — On-chain CLOB (limit + market orders) | IMPLEMENTED | Trading / DeFi | — |
| ZEP-043 | Dated Futures — Fixed-expiry, oracle cash settlement    | IMPLEMENTED | Trading / DeFi | — |
| ZEP-044 | Options — European call/put, writer/buyer, oracle settle | IMPLEMENTED | Trading / DeFi | — |
| ZEP-045 | Meme Coin Launchpad — bonding curve + advanced token (ZbxMemeFactory + ZbxMemeToken) | IMPLEMENTED | Standard / Meme | — |

---

## Categories

| Category | Description                                         |
|----------|-----------------------------------------------------|
| Core     | Consensus, block format, state model changes        |
| Standard | Application-level standards (tokens, naming, etc.)  |
| Meta     | Process, governance, tooling                        |
| Info     | Guidelines, best practices                          |

---

## ZEP Template

Naya ZEP likhne ke liye:
1. `docs/proposals/ZEP-NNN-TITLE.md` file banao
2. Template use karo (see ZEP-001 for example)
3. Pull Request open karo on `zeps` branch
4. Core Team review karega
5. Governance vote (if Core category)