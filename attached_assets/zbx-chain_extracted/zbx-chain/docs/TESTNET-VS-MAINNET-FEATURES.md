# ZBX Chain — Testnet vs Mainnet Feature Matrix

**Last updated:** 2026-06-29 (code-verified — TESTNET_AUDIT_2026-06-29.md)  
**Method:** Direct `.rs` / `.sol` source file reads — no document inference  

---

## Network Identity

| Parameter | Testnet | Mainnet |
|---|---|---|
| Chain ID | **8990** | **8989** |
| Network tag | `ZEBVIX_TESTNET_V1` | `ZEBVIX_MAINNET_V1` |
| RPC (public) | `https://rpc-testnet.zbx.io` | `https://rpc.zbx.io` |
| RPC (local) | `:18545` | `:8545` |
| WS (local) | `:18546` | `:8546` |
| P2P port | `30304` | `30303` |
| BIP-44 coin type | `7878` | `7878` |
| Genesis file | `/etc/zbx/genesis.testnet.json` | `/etc/zbx/genesis.mainnet.json` |
| Validators | 1–5 (dev/test) | 5+ (minimum quorum) |

---

## Infrastructure Config Differences

| Parameter | Testnet | Mainnet |
|---|---|---|
| Max peers | 50 | 100 |
| RocksDB cache | 1024 MB | 2048 MB |
| Mempool max pending | 5,000 | 10,000 |
| Mempool max queued | 2,000 | 5,000 |
| CORS origins | `["*"]` (open) | `["https://zbx.io", "https://app.zbx.io"]` |
| Rate limit | 1,200 RPM | 600 RPM |
| WS RPC | Disabled (opt-in) | Disabled (opt-in) |
| Metrics port | `9001` | `9000` |
| KZG trusted setup | Dev placeholder (τ=1, `ZBX_KZG_ALLOW_DEVNET_TAU=1`) | Official ceremony file required |
| Genesis hash | Operator must pin `TESTNET_GENESIS_HASH` | Operator must pin `MAINNET_GENESIS_HASH` |

---

## Feature Status — Code Verified 2026-06-29

### ✅ Fully Implemented — Testnet AND Mainnet Capable

All features below are confirmed by direct code read — zero stubs in production paths.

| Feature | Crate | Code Evidence |
|---|---|---|
| HotStuff-2 BFT consensus | `zbx-consensus` | 153 pub fns, 32 tests, BLS12-381 aggregation |
| Epoch manager + validator rotation | `zbx-consensus` | `epoch_manager.rs` fully wired |
| VRF-based proposer election | `zbx-consensus` | secp256k1 ECDSA recovery in `vrf_verify()` — **FIXED** |
| BLS Proof-of-Possession | `zbx-staking` | Required at validator registration |
| Slashing (double-sign + evidence) | `zbx-staking` | `EvidenceStore` RocksDB CF, replay window |
| Whistleblower bonds | `zbx-staking` | RocksDB `SlashingBonds` CF — **FIXED** |
| Block rewards + halving | `zbx-rewards` | Halves every 25M blocks, 6 tests |
| Governance (on-chain voting) | `zbx-staking` | `governance.rs` full pipeline, 4 tests |
| Governance RPC | `zbx-rpc` | RocksDB-backed, rehydrated on restart |
| EVM full opcode set (Shanghai) | `zbx-evm` | 140+ opcodes, 30 tests |
| EVM precompiles 0x01–0x09 | `zbx-vm` | ECRECOVER, SHA256, RIPEMD160, IDENTITY, MODEXP, BN128×3, BLAKE2F |
| KZG point eval 0x0A | `zbx-vm` | `kzg::do_kzg_point_eval` EIP-4844 |
| Custom precompiles 0x0B–0x0F | `zbx-evm` | PayID, Oracle, Ed25519, VRF, ZUSD |
| ZVM native opcodes (0xC0–0xCA) | `zbx-zvm` | PAYID, ZKVERIFY, TEEOP, AIINFER dispatch — 32 tests |
| EIP-1559 fee market | `zbx-fee` | Real `BaseFeeCalculator`, 5 tests |
| MPT state trie | `zbx-state` | Real Merkle Patricia Trie, 23 tests |
| RocksDB storage | `zbx-storage` | Atomic `WriteBatch`, crash recovery |
| Transaction mempool | `zbx-mempool` | Full lifecycle, eviction, priority, 34 tests |
| Parallel block execution | `zbx-execution` | Block-STM dependency graph, 20 tests |
| P2P networking | `zbx-network` | libp2p 0.53, Noise XX, Kademlia, 22 tests |
| GossipSub messaging | `zbx-gossip` | Fan-out, LRU cache, peer scorer, 13 tests |
| Chain sync (fast + snap) | `zbx-sync` | Both modes, coordinator, 25 tests |
| Ethereum JSON-RPC | `zbx-rpc` | All `eth_*` + `zbx_*` extensions |
| EIP-4844 KZG blob commitment | `zbx-da` | Real G1 MSM `Σᵢ aᵢ·g1_srs[i]` — **FIXED** |
| KZG blob sampling | `zbx-da` | Real BLS12-381 `multi_miller_loop` |
| Oracle price feeds | `zbx-oracle` | 8 CEX (Binance/Coinbase/Kraken/Gate/Bybit/KuCoin/CoinGecko/Huobi), 81 tests |
| TWAP oracle | `zbx-oracle-twap` | 4-window ring buffer |
| Optimistic oracle (DVM) | `zbx-oracle-optimistic` | Dispute resolution, 9 tests |
| ZK oracle notary | `zbx-oracle-zk` | R1CS + Groth16, 24 tests |
| Threshold DKG / FROST | `zbx-threshold` | Feldman VSS, Horner polynomial, 41 tests |
| RFC 9381 ECVRF | `zbx-crypto` | Full Edwards25519 verify, 6 tests |
| Post-quantum Dilithium-3 | `zbx-crypto` | FIPS 204 — signing |
| Post-quantum Kyber-768 | `zbx-crypto` | FIPS 203 — KEM |
| Groth16 ZK proofs | `zbx-prover` | Real arkworks BN254, 18 tests |
| AMM / DEX pool | `zbx-pool` | Full liquidity ops, 101 tests |
| Perpetuals DEX (ZEP-034) | `zbx-perp` | Open/close/SL/TP/funding/liquidation, 39 tests |
| Lending protocol | `zbx-lending` | Supply/borrow/repay/liquidate/health, 19 tests |
| ZUSD stablecoin | `zbx-contracts` | CDP + stability pool, 101 tests |
| Account Abstraction (ERC-4337) | `zbx-bundler` | Relay, simulation, session keys, 22 tests |
| MEV protection (PBS) | `zbx-mev` | Builder signing, tx ordering, 18 tests |
| Cross-chain bridge | `zbx-bridge` | Rust relayer + Solidity vault |
| XCL cross-chain channels | `zbx-xcl` | Genesis defaults registered — **FIXED** |
| NFT (ZRC-721) | `zbx-nft` | Mint/transfer/royalty/approvals, 20 tests |
| PayID precompile | `zbx-payid` | Human-readable address resolution, 13 tests |
| Payment gateway (ZEP-032) | `zbx-payment` | Invoice lifecycle, 7 tests |
| Confidential transactions (ZEP-025) | `zbx-confidential` | Groth16 shielded transfers, 14 tests |
| Yield optimizer (ZEP-035) | `zbx-yield` | Farming strategies, 20 tests |
| Gaming module (ZEP-031) | `zbx-gaming` | On-chain primitives, 2 tests |
| Token Launchpad (ZEP-036) | `zbx-launchpad` | Bonding curve, 8 tests |
| App store (ZEP-028) | `zbx-appstore` | Registry + deployment, 20 tests |
| Verkle trie (ZEP-021) | `zbx-verkle` | BLS12-381 Pedersen commitments, 8 tests |
| State rent (ZEP-008) | `zbx-state-rent` | Rent-based eviction, 5 tests |
| Light client (ZEP-024) | `zbx-light` | Header chain + proofs, 25 tests |
| WASM contracts | `zbx-wasm` | Runtime integration, 4 tests |
| Partial unbonding / withdraw | `zbx-contracts` | `UnbondingChunk` — **FIXED** |

---

### ⚠️ Testnet Only — Conditional / Not Mainnet Ready

| Feature | Crate | Testnet Status | Mainnet Issue | Blocker ID |
|---|---|---|---|---|
| AI inference (AIINFER `0xCA`) | `zbx-ai-precompile` | ✅ Works with stub weights (`ZBX_AI_ALLOW_STUBS=1`) | Panics without real trained weight files | M-2 |
| KZG trusted setup | `zbx-da` | ✅ Works with `ZBX_KZG_ALLOW_DEVNET_TAU=1` | Real Powers of Tau ceremony required | M-1 |
| PLONK prover | `zbx-zk` | ✅ Fail-closed (`PlonkNotImplemented`) | Same | M-6 |
| Solidity contracts | `contracts/` | ✅ Tested internally (17 Foundry tests) | No external security audit | M-3 |
| WASM contracts | `zbx-wasm` | ✅ Testnet enabled | Not in mainnet config yet | — |
| Codec SSZ/Borsh/SCALE | `zbx-codec` | ✅ Unused in chain (RLP works) | Default stubs remain | M-7 |

---

### ❌ Not Yet Implemented (No Production Code — Spec Only)

These have ZEP proposals but no crate code exists:

| Feature | Proposal | Notes |
|---|---|---|
| ZNS (Zebvix Name Service) | ZEP-037 | No `zbx-zns` crate — PayID covers naming for now |
| Contract factory registry (advanced) | ZEP-038 | Basic factory in Solidity only |
| On-chain raffle | ZEP-039 | Solidity only — no Rust crate |
| Prediction market | ZEP-040 | Solidity only — no Rust crate |
| Card game engine | ZEP-041 | Solidity only — no Rust crate |
| Spot orderbook DEX | ZEP-042 | Design spec only |
| Dated futures | ZEP-043 | Design spec only |
| Options protocol | ZEP-044 | Design spec only |
| Meme factory (advanced) | ZEP-045 | Basic `ZbxMemeFactory.sol` exists |

---

## Testnet Operator Actions (2 Required)

### OB-T1 — Pin Genesis Hash (**Required before first testnet boot**)

```bash
# 1. Build genesis and capture hash
cargo run -p zbx-genesis -- build config/testnet-genesis.json > /tmp/genesis_out.txt
GENESIS_HASH=$(grep "genesis_hash" /tmp/genesis_out.txt | awk '{print $2}')

# 2. Update pinned_genesis.rs
sed -i "s/pub const TESTNET_GENESIS_HASH.*/pub const TESTNET_GENESIS_HASH: H256 = H256($GENESIS_HASH);/" \
    crates/zbx-types/src/pinned_genesis.rs

# 3. Rebuild
cargo build --release -p zbx-node
```

### OB-T2 — KZG Ceremony Bypass (**Required before DA blobs work**)

```bash
# Option A: Devnet bypass (testnet acceptable)
export ZBX_KZG_ALLOW_DEVNET_TAU=1

# Option B: Real ceremony (recommended for persistent testnet)
# Place ceremony file at /etc/zbx/kzg_g2_tau.bin
# File must be: ark-serialize BLS12-381 G2Affine[], 8192 elements
zbx-genesis kzg-init --input powers_of_tau.ptau --output /etc/zbx/kzg_g2_tau.bin
```

---

## Mainnet Launch Blockers

| ID | Issue | Effort |
|---|---|---|
| M-1 | Run Powers of Tau ceremony (or use Ethereum's) — no devnet shortcut | Ceremony (weeks) |
| M-2 | Train + load 12 INT8-quantized AI model weight files via DA layer | Development sprint |
| M-3 | External Solidity security audit by recognized firm | External (months) |
| M-4 | Binary reproducibility: deterministic build + GPG signing pipeline | 1 sprint |
| M-5 | DAO governance contracts deployment on mainnet | Operational |
| M-6 | PLONK prover — optional (Groth16 works; PLONK needed for Solidity verifier) | Medium effort |
| M-7 | Codec SSZ/Borsh/SCALE — implement per-type encoding if cross-chain interop needed | Low effort |
| M-8 | zbx-indexer test coverage — only 1 test currently | 1 sprint |

---

## Previously Listed Open Issues — Now FIXED

These were in the `⚠️ Testnet Only` table in the previous version of this document but are **confirmed fixed by code read**:

| Old Entry | Fix | Verification |
|---|---|---|
| ~~`blob_to_kzg_commitment` SHA-256 not G1 MSM~~ | Real G1 MSM in `zbx-da/src/commitment.rs` | ✅ MB-2 FIXED |
| ~~VRF verify always returns Err~~ | Real secp256k1 ECDSA recovery in `vrf.rs` | ✅ MB-4 FIXED |
| ~~Whistleblower bonds in-memory~~ | RocksDB `SlashingBonds` CF in `persistence.rs` | ✅ MB-5 FIXED |
| ~~`build_tc` zero BLS fallback~~ | Returns `Option<TC>` — no zero BLS propagation | ✅ MB-6 FIXED |
| ~~XCL state `NOT_INITIALIZED`~~ | Genesis defaults registered in `zbx_api.rs` | ✅ M-7 FIXED |
| ~~Partial undelegate amounts trapped~~ | `UnbondingChunk` pushed in `staking_escrow.rs:260` | ✅ FIXED |

---

*Matrix verified: 2026-06-29 via direct source code audit — see `docs/TESTNET_AUDIT_2026-06-29.md`.*
