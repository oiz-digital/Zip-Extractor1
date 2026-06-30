# Zebvix Chain (ZBX) — Technical Whitepaper

**Version:** 1.0.0  
**Date:** 2026-06-28  
**Status:** Testnet-Ready  

---

## Abstract

Zebvix Chain (ZBX) is a Layer-1 blockchain designed for speed, developer experience, and real-world adoption. It combines a HotStuff-2 BFT consensus engine with a full Ethereum Virtual Machine (EVM), a native Zebvix Virtual Machine (ZVM), a ZK proof system, and a data availability layer — all production-ready for public testnet.

Chain IDs: **Mainnet 8989** | **Testnet 8990** | **Devnet 8991**

---

## 1. Introduction

### 1.1 Motivation

Existing Layer-1 blockchains suffer from one or more of the following:

| Problem | Example |
|---------|---------|
| Slow finality (>12s) | Ethereum PoS |
| Poor EVM compatibility | Solana, Aptos |
| Centralized sequencer | many L2s |
| No native stablecoin | most chains |
| Poor developer tooling | niche L1s |

Zebvix Chain addresses all five:
- **2-second block time** with immediate probabilistic finality
- **~99% Ethereum compatibility** (Shanghai spec + EIP-1559)
- **Decentralised HotStuff-2 BFT** — no single sequencer
- **ZUSD** — native algorithmic stablecoin
- **Full developer hub** — RPC, SDK (Rust/JS/Go/Python/Flutter), faucet, contract verification

### 1.2 Design Goals

1. **Safety First** — HotStuff-2 guarantees safety under f < n/3 Byzantine validators
2. **Developer Friendly** — 6 SDKs, GraphQL API, REST API, OpenAPI docs, no-code token creator
3. **EVM Compatible** — deploy any Solidity contract without modification
4. **Scalable** — 1,000+ TPS at testnet, horizontal scaling via DA sharding
5. **Decentralised** — permissionless staking, on-chain governance, open validator set

---

## 2. Consensus

### 2.1 HotStuff-2 BFT

Zebvix Chain uses **HotStuff-2** (two-chain confirmation) consensus:

**Safety:** No two honest validators commit conflicting blocks.  
**Liveness:** The chain makes progress as long as f < n/3 validators are faulty.

**Parameters (Testnet):**
- Validators: up to 100 (top-100 by staked ZBX)
- Quorum: ⌈(2n+1)/3⌉ = 67 votes for n=100
- Block time: 2 seconds
- Epoch length: 172,800 blocks (~4 days)

**Protocol:**
1. **Propose** — Leader broadcasts `Block(height, parent_qc, txs)`
2. **Vote** — Each validator checks safety rules (WAL-persisted) and broadcasts `Vote(block_hash, signature)`
3. **QC formation** — On ≥quorum votes, form QC (BLS aggregate signature)
4. **Commit** — Block at height `h-1` is committed when `h+1` has a valid QC

### 2.2 Validator Set

- **Staking:** Minimum 100,000 ZBX self-stake
- **Delegation:** Any holder can delegate ZBX to a validator
- **Slashing:** 5% slash for double-sign, 0.01%/day for liveness fault
- **Instant jail:** 20 consecutive missed blocks

### 2.3 Epoch Management

Each epoch (172,800 blocks):
1. Stake snapshot at epoch boundary
2. Top-100 by total delegated stake become active validators
3. Epoch key (BLS) rotation
4. Reward distribution (proportional to blocks produced × stake)

---

## 3. Execution Layers

### 3.1 EVM (Ethereum Virtual Machine)

- **Spec:** Cancun / Shanghai compatible
- **Precompiles:** 0x01–0x09 (standard Ethereum) + 0x0A–0x0F (ZBX-native)
- **Gas accounting:** EIP-1559 with ZBX as gas currency
- **Compatibility:** >99% of Solidity 0.8.x contracts deploy unmodified

Custom precompiles (0x0A–0x0F):

| Address | Name | Description |
|---------|------|-------------|
| 0x0A | `kzg_point_eval` | EIP-4844 KZG proof verification |
| 0x0B | `zbx_payid` | PayID → address resolution |
| 0x0C | `zbx_price` | Oracle price feed |
| 0x0D | `ed25519_verify` | Ed25519 signature verification |
| 0x0E | `zbx_vrf` | VRF output + proof verification |
| 0x0F | `zusd_vault` | ZUSD mint/burn interface |

### 3.2 ZVM (Zebvix Virtual Machine)

ZVM extends the EVM with 10 native opcodes:

| Opcode | Hex | Description |
|--------|-----|-------------|
| `PAYID` | `0xF0` | Resolve PayID to address |
| `ZUSDBAL` | `0xF1` | Get ZUSD balance |
| `ZBXPRICE` | `0xF2` | Get ZBX/USD oracle price |
| `ZBXTIME` | `0xF3` | High-resolution timestamp |
| `AASENDER` | `0xF4` | ERC-4337 actual sender |
| `CHAINVER` | `0xF5` | Protocol version |
| `BLOBFEE` | `0xF6` | Current EIP-4844 blob fee |
| `PAYIDSET` | `0xF7` | Register a PayID |
| `ZBXBURN` | `0xF8` | Burn ZBX for fee reduction |
| `ZVMLOG` | `0xF9` | Structured logging |

### 3.3 WASM

WASM smart contracts run alongside EVM contracts. Inter-VM calls are supported via the ZVM precompile dispatcher.

---

## 4. Data Availability

Zebvix Chain implements **EIP-4844 blob transactions** for cheap data availability:

- Each block can include up to 6 blobs (128 KB each) = 768 KB/block
- Blobs are stored on the DA layer (not in state trie) and expire after 30 days
- KZG polynomial commitments ensure data integrity
- Data Availability Sampling (DAS) allows light clients to verify availability

---

## 5. ZK Proof System

### 5.1 Groth16 (BN254)

Production ZK proofs for:
- State transition verification
- Bridge proof verification
- Privacy-preserving payments

Verification: on-chain via `ZbxGroth16Verifier.sol` (BN254 precompiles 0x06/0x07/0x08)

### 5.2 STARK (Goldilocks field)

No-trusted-setup proofs for:
- Layer-2 batch verification
- Rollup state roots

### 5.3 PLONK

PLONK prover: pending `ark-plonk` BN254 stabilisation. Verification is fail-closed until then.

---

## 6. Cross-Chain (XCL)

ZBX native cross-chain layer (ZEP-026) implements an IBC-compatible protocol:

- **Channels:** Ordered and unordered message channels
- **Light clients:** BLS QC + MPT proof verification
- **Token transfers:** FT-1 fungible token protocol

Testnet bridges:
- **Ethereum Sepolia** — 3-of-5 multisig (ECDSA relayers)
- **BNB Testnet** — 3-of-5 multisig
- **Polygon Amoy** — 3-of-5 multisig

---

## 7. Native Features

### 7.1 PayID

Human-readable addresses (`alice.zbx` → `0xAbC...`):
- Resolved on-chain via ZVM `PAYID` opcode
- PayID registry stored in state trie
- Subdomain support (`alice.mywallet.zbx`)

### 7.2 ZUSD

Algorithmic stablecoin pegged to USD:
- Backed by ZBX collateral (150% minimum collateral ratio)
- Mint: lock ZBX → receive ZUSD
- Burn: return ZUSD → unlock ZBX
- Stability mechanism: oracle-guided collateral ratio enforcement

### 7.3 Oracle

Multi-source price feeds with TWAP:
- 7 external sources for ZBX/USD price
- Median aggregation (outlier-resistant)
- TWAP accumulator for DEX integration
- On-chain circuit breaker (pauses if price moves >20% in 1 hour)

### 7.4 Governance

On-chain governance:
- Proposal types: Parameter change, Upgrade, Treasury spend, Validator slash, App suspension
- Quorum: 4% of staked ZBX
- Voting period: 7 days
- Timelock: 48 hours before execution

---

## 8. Developer Experience

### 8.1 SDKs

| SDK | Language | Status |
|-----|----------|--------|
| zbx-sdk | Rust | ✅ Production |
| zebvix-js | TypeScript | ✅ Production |
| zbx-go | Go | ✅ Production |
| zbx-python | Python | ✅ Production |
| zbx-flutter | Flutter/Dart | ✅ Production |

### 8.2 APIs

| API | Protocol | Port | Description |
|-----|----------|------|-------------|
| JSON-RPC | HTTP | 8545 | Ethereum-compatible eth_* + zbx_* |
| WebSocket | WS | 8546 | Subscriptions (newHeads, pendingTxs, logs) |
| GraphQL | HTTP/WS | 8547 | Query + subscription API |
| REST | HTTP | 8548 | OpenAPI 3.1 REST endpoints |
| Trace | HTTP | 8549 | debug_traceTransaction, debug_traceCall |

### 8.3 No-Code Tools

| Tool | Contract | Description |
|------|----------|-------------|
| ZRC-20 Creator | ZRC20Creator.sol | Deploy fungible tokens |
| ZRC-721 Creator | ZRC721Creator.sol | Deploy NFT collections |
| ZRC-1155 Creator | ZRC1155Creator.sol | Deploy multi-token contracts |
| DAO Creator | DAOCreator.sol | Deploy governance DAOs |

### 8.4 Developer Hub

URL: `https://dev.zebvix.com`

Features:
- API key management
- Testnet faucet (1 ZBX/address/day)
- Contract verification
- RPC dashboard (latency, rate limits, error rates)
- Chain analytics
- SDK downloads
- Interactive documentation

---

## 9. Security

### 9.1 Cryptographic Primitives

| Primitive | Usage | Library |
|-----------|-------|---------|
| BLS12-381 | Consensus aggregate signatures | `blst` |
| secp256k1 | EVM transaction signing, VRF, bridge | `k256` |
| Ed25519 | Native ZBX transaction signing | `ed25519-dalek` |
| Keccak-256 | Address derivation, trie, domain tags | `sha3` |
| Groth16/BN254 | ZK proofs | `arkworks` |
| RFC 9381 ECVRF | Block proposer election | `curve25519-dalek` |
| Dilithium-3 | Post-quantum signing (FIPS 204) | `pqcrypto-dilithium` |
| Kyber-768 | Post-quantum KEM (FIPS 203) | `pqcrypto-kyber` |
| Noise XX | P2P transport encryption | `libp2p 0.53` |

### 9.2 Replay Protection

- **EIP-155** — chain ID in transaction hash
- **Network domain tags** — `ZEBVIX_TESTNET_V1\x00` prefix for off-chain signatures
- **Bridge nonces** — per-sender nonces on both sides
- **Session key binding** — ERC-4337 paymaster signatures scoped to chain + domain

### 9.3 Anti-Spam / Anti-DoS

- EIP-1559 fee market — economic spam prevention
- Mempool rate limiting (max txs per sender per block)
- RPC rate limiting (per API key, per IP)
- P2P peer scoring (ban misbehaving peers)

---

## 10. Tokenomics

**Token:** ZBX (Zebvix)  
**Decimals:** 18  
**Total Supply:** 1,000,000,000 ZBX  

| Allocation | % | Vesting |
|-----------|---|---------|
| Staking Rewards | 30% | Continuous emission over 10 years |
| Foundation | 20% | 4-year linear vesting, 1-year cliff |
| Ecosystem Fund | 20% | Governance-controlled |
| Team | 15% | 4-year linear vesting, 1-year cliff |
| Public Sale | 10% | Immediate |
| Advisors | 5% | 2-year linear vesting |

---

## 11. Testnet

**Chain ID:** 8990  
**RPC:** `https://testnet-rpc.zebvix.com`  
**Explorer:** `https://testnet-explorer.zebvix.com`  
**Faucet:** `https://dev.zebvix.com/faucet`  

**Genesis validators:** 4 (quorum=3, tolerates f=1)  
**Genesis faucet:** 10,000,000 ZBX  

**Testnet readiness:** ~99% (2 operator actions pending before boot — see AUDIT_2026-06-28.md)

---

## 12. Roadmap

### Phase 1 (Testnet — Current)
- ✅ HotStuff-2 consensus
- ✅ Full EVM (Cancun spec)
- ✅ ZVM with 10 native opcodes
- ✅ PayID, ZUSD, Oracle, Staking, Bridge
- ✅ ZK proofs (Groth16 + STARK)
- ✅ Data availability (EIP-4844)
- ✅ 5 SDKs (Rust, TS, Go, Python, Flutter)
- ✅ GraphQL + REST API
- ✅ No-code token/DAO creator
- ✅ App Store registry
- ✅ Developer Hub

### Phase 2 (Mainnet Preparation)
- PLONK prover (pending `ark-plonk` BN254 stabilisation)
- External security audit (Solidity contracts)
- Verkle tree migration (block 200,000)
- AI Cloud (inference marketplace)
- AI Agent SDK
- Binary reproducibility + GPG signing

### Phase 3 (Mainnet)
- KZG ceremony (real trusted setup)
- AI model weights (INT8 quantized)
- 100-validator active set
- L2 rollup integration

---

## References

1. HotStuff: BFT Consensus with Linearity and Responsiveness — Yin et al. 2019
2. EIP-1559: Fee Market Change — Buterin et al. 2019
3. EIP-4844: Shard Blob Transactions — Proto-danksharding
4. Ethereum Yellow Paper — Wood 2014
5. BIP-32/39/44 — Bitcoin HD wallet standards
6. ERC-4337: Account Abstraction — Buterin et al. 2021
7. IBC Protocol Specification — Cosmos Foundation 2020
8. Noise Protocol Framework — Perrin 2018

---

*This whitepaper describes the Zebvix Chain testnet implementation as of 2026-06-28.*  
*Source code: https://github.com/zebvix/zbx-chain*
