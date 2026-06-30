# Zebvix Chain Architecture

## Overview

Zebvix Chain (ZBX) is a high-performance, EVM-compatible Layer-1 blockchain built in Rust.
It uses HotStuff-BFT consensus for fast, deterministic finality (5-second target block time)
and supports full Ethereum JSON-RPC compatibility for easy toolchain integration.

**Key parameters**

| Parameter | Value |
|---|---|
| Chain ID (mainnet) | 8989 |
| Chain ID (testnet) | 8990 |
| Block time | 5 seconds |
| Hard cap | 150,000,000 ZBX |
| Validator min stake | 100 ZBX |
| Consensus | HotStuff-BFT (BLS12-381 aggregate signatures, 2f+1 quorum) |
| EVM | London + Shanghai (EIP-3855 PUSH0, EIP-1153 transient storage, EIP-5656 MCOPY) |

---

## Crate Structure

### Core (62 total crates — those used directly by the node binary)

| Crate | Purpose |
|---|---|
| `zbx-types` | Core primitives: `Address`, `H256`, `U256`, `Block`, `Transaction`, `AccountState`, `Receipt` |
| `zbx-crypto` | BLS12-381 (keys, sign, verify, aggregate), secp256k1 (ECDSA, `to_address`), Keccak-256, VRF, Merkle |
| `zbx-storage` | RocksDB-backed storage — blocks, state, receipts, genesis, chain metadata |
| `zbx-state` | World-state DB with Merkle-Patricia Trie (`compute_state_root`), snapshot/revert |
| `zbx-mempool` | Gas-price-priority tx pool — pending + queued sets, EIP-1559 replacement rules |
| `zbx-execution` | Block-STM parallel executor (Rayon) + sequential fallback, MVCC conflict detection |
| `zbx-evm` | EVM interpreter — London + Shanghai opcodes, 9 precompiles, EIP-1559 gas metering |
| `zbx-vm` | Higher-level VM wrappers, journaled state, transient storage (EIP-1153) |
| `zbx-consensus` | HotStuff-BFT — votes, QCs, safety rules (locked-QC), pacemaker, VRF proposer selection |
| `zbx-network` | P2P TCP transport, Noise XX handshake, JSON message framing, peer registry |
| `zbx-rpc` | JSON-RPC 2.0 HTTP + WebSocket — `eth_*`, `zbx_*`, `net_*`, `web3_*` |
| `zbx-staking` | PoS validator registry, delegation, rewards, slashing (5% stake burn), epoch rotation |
| `zbx-bridge` | Ethereum/BSC/Polygon bridge — Merkle proofs, 3-of-5 multisig, relayer |
| `zbx-xcl` | Native Cross-Chain Layer — trustless BLS light-client proofs, IBC-style channels |
| `zbx-metrics` | Prometheus `/metrics` endpoint (port 9001) |
| `zbx-genesis` | Genesis block construction, alloc parsing, validator initialisation |
| `zbx-wasm` | WASM smart contract runtime (runs alongside EVM) |
| `zbx-zk` | ZK verifier — Groth16 over BN254, PLONK (fail-closed), STARK (Goldilocks field, FRI, no trusted setup) |
| `zbx-zvm` | ZK VM — programmable validity proof execution |
| `zbx-threshold` | Threshold BLS (FROST) + BLS12-381 aggregate signatures (ValidatorBitmap, PoP, BLSQuorumCertificate, batch verify) |
| `zbx-da` | Data availability layer (EIP-4844 blob transactions, KZG, DAS) |
| `zbx-oracle` | **[Session 40 — Advanced Oracle Suite]** Decentralized price oracle — **14 feeds** (ZBX/USD, ZUSD/USD, ZNS/USD, ETH/USD, BTC/USD, BNB/USD, SOL/USD, AVAX/USD, MATIC/USD, ARB/USD, OP/USD, LINK/USD, DOT/USD, USD/INR); **8 CEX+aggregator sources** (Binance, Coinbase, Kraken, Gate.io, Bybit, KuCoin, CoinGecko, CoinMarketCap); **8 EVM networks** via ZBX-XCM relay (ZBX mainnet/testnet + ETH/BSC/Polygon/Arbitrum/Optimism/Avalanche); **7 advanced modules**: TWAP ring buffer, circuit breaker FSM, multi-chain relay, DEX fetcher (Uniswap V3/PancakeSwap/ZBX DEX), reporter slasher, heartbeat monitor, Merkle price proof; Chainlink `AggregatorV3Interface` compatible on all relay chains |
| `zbx-pool` | Native AMM — Uniswap v2 constant-product formula with 10-layer security (reentrancy guard, circuit breaker, slippage, deadline, oracle deviation, price impact cap, k-invariant); 1 canonical genesis pool: ZBX/ZUSD (0.30%); multi-hop router (1-hop + 2-hop) |
| `zbx-pq` | **[NEW — ZEP-015]** Post-quantum cryptography: CRYSTALS-Dilithium-3 (NIST FIPS 204 ML-DSA-65, real lattice math via `fips204` crate), Kyber-768 KEM, ECDSA+PQ hybrid transition (3 phases); PrivKey zeroized on drop |
| `zbx-confidential` | **[NEW — ZEP-025]** Confidential transactions: Pedersen commitments over Ristretto255 (curve25519-dalek v4), ERC-5564 stealth addresses (dual-key: spend+view), Bulletproofs sigma-protocol range proofs; BlindingFactor zeroized on drop |
| `zbx-contracts` | **[ZEP-006 — S38]** ZRC-20 v1.1 runtime: `zrc20_token` (single-token state engine — freeze, native lock, mint-flags, 2-step ownership, anti-bot, hooks, 42 tests); all public types re-exported at crate root |

### Node binary (`node/`)

| Module | File | Role |
|---|---|---|
| Entry point | `src/main.rs` | CLI (`clap`), logging init, network selection, `ZbxNode::run()` |
| Node orchestrator | `src/node.rs` | Assembles all subsystems, spawns tokio tasks, wires broadcast channels |
| Config | `src/config.rs` | `NodeConfig` (TOML-parseable): `ChainConfig` (with `extra_validators`), `NetworkConfig`, `RpcConfig`, `ConsensusConfig`, `StorageConfig`, `MetricsConfig` |
| Genesis | `src/genesis.rs` | `GenesisConfig`, `GenesisAlloc`, `Network` enum, `BootstrapPolicy`, genesis fail-fast (strict hash match) |
| Network | `src/network.rs` | `NetworkServer` — TCP listen, bootnode dial, TX relay, peer discovery, block sync, exponential backoff reconnect |
| Consensus driver | `src/consensus.rs` | `ConsensusDriver` — vote broadcast, block commit, network wiring |
| Block producer | `src/block_producer.rs` | `ProducerConfig`, `produce_one()` — VRF proposer, mempool drain, parallel execution |
| **Key tool** | `src/bin/zbx-keygen.rs` | BLS + secp256k1 keypair generator for validators — text + JSON output, genesis snippet printer |

### Binaries

| Binary | Source | Purpose |
|---|---|---|
| `zbx-node` | `node/src/main.rs` | Full node — starts RPC, P2P, consensus |
| `zbx-keygen` | `node/src/bin/zbx-keygen.rs` | Validator keypair generator — BLS + secp256k1 |

---

## Block Production

1. Pacemaker timer fires → round begins
2. VRF selects block proposer from active validator set
3. Proposer collects pending txs from mempool → Block-STM parallel execution
4. Proposer broadcasts proposal + parent QC to all peers via TCP
5. Validators verify safety rules (locked-QC) → cast BLS-signed `Prepare` vote
6. Proposer aggregates 2f+1 votes → `Prepare QC` → broadcasts
7. Validators cast `PreCommit` vote on Prepare QC
8. Proposer aggregates → `PreCommit QC` → broadcasts
9. Validators cast `Commit` vote on PreCommit QC
10. **DECIDE** — block final → `execute_and_commit()` → RocksDB → `broadcast_block()` to peers

---

## P2P Network

- **Transport**: raw TCP — JSON-framed messages (4-byte big-endian length prefix)
- **Encryption**: Noise XX handshake on every connection (authenticated + encrypted)
- **Discovery**: `FindPeers`/`Peers` exchange on connect; newly discovered peers dialled independently
- **Reconnect**: exponential backoff 5s → 120s cap, loops forever
- **TX relay**: `eth_sendRawTransaction` → mempool accept → `broadcast::Sender<SignedTransaction>` → `Message::Transaction` to all peers
- **Block sync**: auto `GetBlockRange` on connect if peer is ahead; continuous pipeline on import

**Message types** (defined in `zbx-network/src/messages.rs`):

| Message | Purpose |
|---|---|
| `Ping` / `Pong` | Keep-alive |
| `GetBlockRange { from, to }` | Block sync request |
| `Block(Block)` / `Blocks(Vec<Block>)` | Block delivery |
| `GetBlockByHash(H256)` | Fetch specific block |
| `Vote(Vote)` | HotStuff-BFT vote propagation |
| `Transaction(SignedTransaction)` | TX relay after mempool accept |
| `Transactions(Vec<SignedTransaction>)` | Batch TX relay |
| `FindPeers { target }` | Peer discovery |
| `Peers(Vec<String>)` | Peer address response |

---

## Consensus Safety

- One-vote-per-round rule enforced by `SafetyRules` (persisted to disk)
- Locked-QC rule: can only vote for blocks extending the locked chain
- BLS12-381 aggregate signatures — 2f+1 quorum threshold
- Slashing for double-sign: 5% stake burn + permanent jail
- Genesis fail-fast: `bootstrap_into` hard-fails if on-disk genesis hash differs from config (`BootstrapPolicy::StrictFailFast`)

---

## EVM Compatibility

- Ethereum London + Shanghai opcode set (PUSH0, MCOPY, transient storage)
- All 9 standard precompiled contracts (ecrecover, SHA256, RIPEMD160, etc.)
- EIP-1559 fee model (base fee burned + priority tip to validators)
- EIP-2930 access lists, EIP-155 replay protection
- `eth_*` JSON-RPC namespace — MetaMask / Hardhat / Foundry / viem support

> **Known limitation (S7-EVM3, CRITICAL, OPEN):** CALL/DELEGATECALL/STATICCALL/CREATE/CREATE2/REVERT
> match arms are not yet implemented in `interpreter.rs`. Multi-contract Solidity dApps will fail.
> Single-contract deploys and direct ZBX transfers work.

---

## Block-STM Parallel Executor (`zbx-execution/src/parallel.rs`)

- **Phase 1**: Rayon `par_iter()` — speculative execution, reads from MVCC `MvBalanceTable`
- **Phase 2**: O(n²) R-W conflict detection (pairwise `ReadWriteSet` comparison)
- **Phase 3**: Sequential re-execution of aborted txs in original order
- Configurable Rayon thread pool via `num_threads`

---

## Security Features

- Noise XX handshake for encrypted + authenticated P2P connections
- Rate limiting on JSON-RPC (600 req/min default, testnet 1200)
- Merkle proof verification for bridge deposits
- `zeroize` on drop for all private key material
- `#[deny(unsafe_code)]` in all crates except `zbx-crypto`
- `cargo audit` + `cargo deny` in CI

---

## Genesis Startup Flow

```
zbx-node --network testnet --config node/configs/testnet.toml
   │
   ├── NodeConfig::from_file(testnet.toml)
   │
   ├── chain.genesis_file exists? → GenesisConfig::from_file(genesis.json)
   │                          no → GenesisConfig::for_network(Testnet) [hardcoded preset]
   │
   ├── bootstrap_into(db, StrictFailFast)
   │     ├── DB empty?     → write genesis block + alloc accounts → Ok((true, hash))
   │     └── DB has chain? → verify hash match → mismatch → HARD FAIL (use --allow-chain-mismatch for recovery)
   │                                           → match      → Ok((false, hash))
   │
   └── ZbxNode::run() — start RPC + P2P + consensus tasks
```

---

## Config Files

| File | Purpose |
|---|---|
| `node/configs/testnet.toml` | **Production testnet config** — chain_id 8990, RPC 18545, P2P 30304 |
| `node/configs/mainnet.toml` | Production mainnet config — chain_id 8989, RPC 8545, P2P 30303 |
| `config/testnet-genesis.json` | Testnet genesis — validators + alloc (balances as quoted decimal strings) |
| `config/mainnet-genesis.json` | Mainnet genesis |
