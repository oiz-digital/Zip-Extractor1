# Zebvix Chain (ZBX)

**Zebvix Technologies Pvt Ltd** — EVM-compatible Layer-1 blockchain built in Rust.

> **Status (2026-06-29):** Devnet ✅ Ready | Testnet ✅ **Launch Ready (~99%)** | Mainnet ❌ Not Ready (~48%)  
> Full audit: [docs/TESTNET_AUDIT_2026-06-29.md](./docs/TESTNET_AUDIT_2026-06-29.md)

---

## Chain Specs

| | |
|--|--|
| Chain ID | 8989 (mainnet) / 8990 (testnet) |
| Token | ZBX (18 decimals) |
| Address format | 20-byte EVM-style (Keccak256(pubkey)[12:]) |
| Signing | Ed25519 + ECDSA (EIP-155) |
| Block time | 5 seconds |
| Total supply cap | 150,000,000 ZBX |
| Foundation pre-mine | 9,990,000 ZBX (6.66%) |
| AMM genesis seed | 20,000,000 ZBX (13.33%) |
| Block-mined supply | 120,010,000 ZBX (80.01%) |
| Initial block reward | 3 ZBX (halves every 25M blocks ≈ 3.96 years) |
| Min validator stake | 100 ZBX |
| Min delegator stake | 10 ZBX |
| Unbonding period | 7 days |
| Consensus | HotStuff-2 BFT (ZEP-022) |
| EVM | Shanghai + precompiles 0x01–0x0a + custom precompiles 0x0A–0x0F |
| Storage | RocksDB |
| RPC | Ethereum-compatible JSON-RPC (HTTP + WebSocket) |
| Post-quantum | Dilithium-3 (FIPS 204) + Kyber-768 (FIPS 203) |

---

## Technology Stack

| Layer | Technology |
|---|---|
| Language | Rust 2021 edition |
| Async runtime | Tokio 1.x |
| Consensus | HotStuff-2 (ZEP-022) — 153 pub fns, BLS12-381 aggregation |
| EVM | Custom `zbx-evm` (Shanghai), `zbx-zvm` (native opcodes 0xC0–0xCA) |
| Storage | RocksDB atomic WriteBatch + crash recovery |
| Crypto | blst (BLS12-381), ed25519-dalek, k256, sha3, arkworks |
| P2P | libp2p 0.53 + Noise XX + Kademlia |
| ZK | arkworks Groth16/Bn254 (PLONK: fail-closed pending ark-plonk) |
| Post-quantum | Dilithium-3 (FIPS 204), Kyber-768 (FIPS 203) |
| Oracle | 8 CEX sources + TWAP + ZK notary |

---

## Build

```bash
# Ubuntu/Debian
apt-get install -y build-essential clang pkg-config libssl-dev librocksdb-dev
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source $HOME/.cargo/env

git clone https://github.com/zebvix/zbx-chain
cd zbx-chain
cargo build --release --features zvm
```

### Feature Flags

| Flag | Effect |
|---|---|
| `zvm` | Enable ZVM native opcodes (PAYID, AIINFER, ZKVERIFY, etc.) |
| `testnet` | Testnet chain ID (8990) + zero-value premine |

---

## Quick Start — Devnet (Single Validator)

```bash
# 1. Generate validator keypair
zbx-node keygen --out ~/.zbx/validator.key

# 2. Start devnet node
zbx-node start \
  --home ~/.zbx \
  --config node/configs/devnet.toml \
  --rpc 0.0.0.0:8545

# 3. Check sync status
curl -s http://localhost:8545 \
  -d '{"jsonrpc":"2.0","method":"eth_blockNumber","id":1}' | jq
```

## Quick Start — Testnet

Two operator actions required before first boot:

```bash
# 1. Pin the genesis hash (required — node refuses to start without it)
cargo run --release -p zbx-genesis -- build config/testnet-genesis.json \
  | tee /tmp/genesis.log
# Update TESTNET_GENESIS_HASH in crates/zbx-types/src/pinned_genesis.rs
# then rebuild: cargo build --release -p zbx-node

# 2. Set KZG environment variable (required — DA blobs need this)
export ZBX_KZG_ALLOW_DEVNET_TAU=1

# 3. Set AI stub flag (testnet-safe)
export ZBX_AI_ALLOW_STUBS=1
export ZBX_CHAIN_ENV=testnet

# 4. Start
zbx-node start --config config/testnet.toml
```

See [docs/TESTNET_LAUNCH_GUIDE.md](./docs/TESTNET_LAUNCH_GUIDE.md) for the full step-by-step operator guide.

---

## Repository Layout

```
zbx-chain/
├── crates/          # 75 Rust crates (core chain, DeFi, tooling)
├── node/            # Production binary + configs (devnet/testnet/mainnet)
├── contracts/       # 133 Solidity files + 40 interfaces + 17 Foundry tests
├── sdk/
│   ├── zebvix-js/  # TypeScript SDK
│   └── ethers-zbx/ # ethers.js extension
├── tests/           # Integration, unit, property tests
├── docs/            # All documentation (86 files)
├── k8s/             # Kubernetes manifests (13 files)
├── monitoring/      # Prometheus + Grafana dashboards
├── scripts/         # Deploy, keygen, benchmark, CI scripts
├── fuzz/            # 10 cargo-fuzz targets
└── proto/           # Protocol Buffers (consensus, DA, prover)
```

---

## Key Documents

| Document | Purpose |
|---|---|
| [docs/TESTNET_AUDIT_2026-06-29.md](./docs/TESTNET_AUDIT_2026-06-29.md) | Latest code-verified testnet audit (75 crates) |
| [docs/TESTNET_LAUNCH_GUIDE.md](./docs/TESTNET_LAUNCH_GUIDE.md) | Step-by-step testnet operator guide |
| [docs/TESTNET-VS-MAINNET-FEATURES.md](./docs/TESTNET-VS-MAINNET-FEATURES.md) | Feature matrix — what works where |
| [docs/CODE_GAPS.md](./docs/CODE_GAPS.md) | Open gaps + all fixes history |
| [docs/MAINNET_LAUNCH_CHECKLIST.md](./docs/MAINNET_LAUNCH_CHECKLIST.md) | Mainnet go/no-go criteria |
| [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md) | System architecture + subsystem map |
| [docs/VALIDATOR_GUIDE.md](./docs/VALIDATOR_GUIDE.md) | Validator setup, key management, monitoring |
| [docs/API_REFERENCE.md](./docs/API_REFERENCE.md) | Full eth_* + zbx_* RPC reference |
| [docs/proposals/ZEP-000-INDEX.md](./docs/proposals/ZEP-000-INDEX.md) | All 45 ZEP proposals index |
| [SECURITY.md](./SECURITY.md) | Security policy + responsible disclosure |
| [CHANGELOG.md](./CHANGELOG.md) | Release history |
| [deploy/DEPLOY_GUIDE.md](./deploy/DEPLOY_GUIDE.md) | Production deployment guide |

---

## Custom Precompiles

ZBX extends the EVM with native precompiles at addresses `0x0A`–`0x0F`:

| Address | Name | Description | Status |
|---|---|---|---|
| `0x0A` | PayID | Human-readable address resolution | ✅ Testnet + Mainnet |
| `0x0B` | KZG Point Eval | EIP-4844 KZG verification | ✅ Testnet + Mainnet |
| `0x0C` | Price Oracle | On-chain price feed (8 CEX sources) | ✅ Testnet + Mainnet |
| `0x0D` | Ed25519 Verify | Ed25519 signature verification | ✅ Testnet + Mainnet |
| `0x0E` | VRF Verify | RFC 9381 ECVRF-EDWARDS25519-SHA512-ELL2 | ✅ Testnet + Mainnet |
| `0x0F` | ZUSD Vault | Stablecoin vault state read | ✅ Testnet + Mainnet |

ZVM additionally adds opcodes `0xC0`–`0xCA` including `AIINFER` (on-chain AI inference, stub weights on testnet).

---

## Mainnet Readiness

Full details: [docs/TESTNET_AUDIT_2026-06-29.md](./docs/TESTNET_AUDIT_2026-06-29.md)

### ✅ Fixed (code-verified 2026-06-27 to 2026-06-29)

| ID | Description | Fixed |
|---|---|---|
| MB-2 | `blob_to_kzg_commitment` — real G1 MSM (`Σᵢ aᵢ·g1_srs[i]`) | 2026-06-27 |
| MB-4 | Consensus VRF verify — secp256k1 ECDSA-backed, 7 tests | 2026-06-27 |
| MB-5 | Whistleblower bonds — RocksDB `SlashingBonds` CF | 2026-06-27 |
| MB-6 | `build_tc` returns `Option` — zero BLS TC cannot propagate | 2026-06-27 |
| — | XCL cross-chain state `NOT_INITIALIZED` at genesis | 2026-06-28 |
| — | Partial undelegate amounts trapped (`UnbondingChunk` wired) | 2026-06-28 |

### ❌ Open — Mainnet Blockers (not testnet blockers)

| ID | Description | Notes |
|---|---|---|
| M-1 | **KZG Powers of Tau ceremony** | External ceremony required; testnet uses `ZBX_KZG_ALLOW_DEVNET_TAU=1` |
| M-2 | **AI model weights** (12 INT8-quantized models) | Testnet uses stub weights (`ZBX_AI_ALLOW_STUBS=1`); mainnet panics without real files |
| M-3 | **External Solidity security audit** | Internal tests pass (17 Foundry tests); 3rd-party audit needed before mainnet |

---

## RPC Endpoints

Base URL: `http://NODE_IP:8545` (mainnet) · `http://NODE_IP:18545` (testnet)  
WebSocket: `ws://NODE_IP:8546` (mainnet) · `ws://NODE_IP:18546` (testnet)

| Method | Description |
|---|---|
| `eth_blockNumber` | Current chain height |
| `eth_getBalance` | Account ZBX balance |
| `eth_sendRawTransaction` | Submit signed transaction |
| `eth_call` | Call smart contract (read-only) |
| `eth_gasPrice` | Current effective gas price |
| `eth_feeHistory` | EIP-1559 fee history |
| `zbx_getValidators` | Active validator set |
| `zbx_proposeGovernance` | Submit governance proposal |
| `zbx_getGovernanceProposal` | Retrieve proposal by ID |
| `zbx_getOraclePrice` | Current oracle price for a feed |
| `zbx_validatorStatus` | Validator health + uptime |

Full reference: [docs/API_REFERENCE.md](./docs/API_REFERENCE.md)

---

## ZEP Proposals (45 Total)

| Range | Status |
|---|---|
| ZEP-001 – ZEP-026 | ✅ All implemented (code-verified) |
| ZEP-027 – ZEP-030 | ACCEPTED (docs present in `docs/proposals/`) |
| ZEP-031 – ZEP-036 | ✅ All implemented (code-verified) |
| ZEP-037 – ZEP-045 | Solidity-only or spec — Rust crates pending |

Full index: [docs/proposals/ZEP-000-INDEX.md](./docs/proposals/ZEP-000-INDEX.md)

---

## License

MIT — see `LICENSE` file.

© 2026 Zebvix Technologies Pvt Ltd
