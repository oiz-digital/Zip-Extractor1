# ZBX Chain — Testnet Launch Guide

**Version:** 1.0  
**Date:** 2026-06-29  
**Audience:** Validators / Node operators launching or joining the Zebvix Testnet  
**Chain ID:** 8990  
**Status:** Testnet-ready — 2 operator actions required before boot  

---

## Prerequisites

| Requirement | Minimum | Recommended |
|---|---|---|
| CPU | 4 cores | 8 cores |
| RAM | 8 GB | 16 GB |
| Disk | 100 GB SSD | 500 GB NVMe |
| Network | 100 Mbps | 1 Gbps |
| OS | Ubuntu 22.04 / Debian 12 | Ubuntu 22.04 LTS |
| Rust | 1.78+ stable | latest stable |
| Open ports | 30304 (P2P), 18545 (RPC) | + 9001 (metrics) |

---

## Step 1 — Build the Node

```bash
# Clone the repository
git clone https://github.com/zebvix/zbx-chain.git
cd zbx-chain

# Install Rust (if not installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env

# Build release binary
cargo build --release -p zbx-node

# Verify build
./target/release/zbx-node --version
# Expected: zbx-node 1.0.0-testnet (chain_id=8990)
```

---

## Step 2 — Generate Validator Keys

```bash
# Generate a new secp256k1 + Ed25519 + BLS12-381 keypair
./target/release/zbx-node keygen generate \
    --output ./keystore/validator.json \
    --password-file ./keystore/password.txt

# This creates:
#   validator.json     — encrypted keystore (secp256k1 + BLS12-381)
#   validator.pub.json — public keys (safe to share)
#   address.txt        — derived address (0x...)

# IMPORTANT: Back up ./keystore/ securely.
# The BLS key is used for block signing — losing it means slashing risk.
```

---

## Step 3 — Pin the Genesis Hash (REQUIRED — OB-T1)

This step is mandatory. Without it the node will refuse to start.

```bash
# 3a. Build genesis and extract hash
cargo run --release -p zbx-genesis -- build config/testnet-genesis.json \
    | tee /tmp/genesis_build.log

GENESIS_HASH=$(grep "genesis_hash:" /tmp/genesis_build.log | awk '{print $2}')
echo "Genesis hash: $GENESIS_HASH"

# 3b. Patch pinned_genesis.rs with the real hash
# Replace the SENTINEL ([0xFF;32]) with the actual hash
cargo run --release -p zbx-cli -- genesis pin \
    --crate-path crates/zbx-types/src/pinned_genesis.rs \
    --chain testnet \
    --hash "$GENESIS_HASH"

# 3c. Rebuild with pinned hash
cargo build --release -p zbx-node

# 3d. Verify pin is set
cargo test -p zbx-types testnet_genesis_hash_is_not_sentinel
# PASS = correctly pinned
```

> **Note:** If you are joining an existing testnet (not launching it), the binary you received already has `TESTNET_GENESIS_HASH` pinned. Skip 3a-3c and just run 3d to verify.

---

## Step 4 — Configure KZG Trusted Setup (REQUIRED — OB-T2)

Choose one option:

### Option A — Devnet bypass (fast, testnet-acceptable)

```bash
# Set env var before starting node
export ZBX_KZG_ALLOW_DEVNET_TAU=1
```

This will print:
```
WARN zbx_da::commitment — DEVNET placeholder (G₂_τ = G₂, τ=1). 
     Do not use on mainnet. KZG proofs are insecure (τ is known).
```
This warning is **expected and correct** for testnet.

### Option B — Real ceremony file (persistent testnet, more secure)

```bash
# Convert a Powers of Tau file (e.g. from Ethereum's KZG ceremony)
./target/release/zbx-node genesis kzg-init \
    --input /path/to/powersOfTau28_hez_final_18.ptau \
    --output /etc/zbx/kzg_g2_tau.bin \
    --curve bn254

# Set file path in config
echo 'kzg_ceremony_path = "/etc/zbx/kzg_g2_tau.bin"' >> config/testnet.toml
```

---

## Step 5 — Configure the Node

Edit `config/testnet.toml`:

```toml
[network]
chain_id = 8990
listen_addr = "0.0.0.0:30304"
bootstrap_peers = [
    "/dns4/boot1.testnet.zbx.io/tcp/30304/p2p/12D3KooW...",
    "/dns4/boot2.testnet.zbx.io/tcp/30304/p2p/12D3KooW...",
]

[rpc]
http_port = 18545
ws_port = 18546
cors_origins = ["*"]        # Open on testnet
rate_limit_rpm = 1200

[validator]
keystore_path = "./keystore/validator.json"
keystore_password_file = "./keystore/password.txt"
bls_key_path = "./keystore/bls.json"

[storage]
data_dir = "/var/lib/zbx/testnet"
rocksdb_cache_mb = 1024

[metrics]
enabled = true
port = 9001

[da]
# Only needed if NOT using ZBX_KZG_ALLOW_DEVNET_TAU=1
# kzg_ceremony_path = "/etc/zbx/kzg_g2_tau.bin"
```

---

## Step 6 — Set Environment Variables

```bash
# Required
export ZBX_CHAIN_ENV=testnet
export ZBX_KZG_ALLOW_DEVNET_TAU=1        # or point to real ceremony file

# Recommended
export ZBX_AI_ALLOW_STUBS=1             # AI inference stub weights (testnet safe)
export RUST_LOG=info,zbx_consensus=debug  # Log level

# Optional — INR price feed
export ORACLE_AI_API_KEY=your_real_api_key  # Without this, INR/USD feed is disabled
```

---

## Step 7 — Register as Validator (Testnet Faucet)

```bash
# 7a. Get testnet ZBX from faucet
curl -X POST https://faucet.testnet.zbx.io/v1/fund \
    -H "Content-Type: application/json" \
    -d '{"address": "0xYOUR_ADDRESS", "amount": "100000000000000000000000"}'
# Sends 100,000 ZBX (min stake: 1,000 ZBX)

# 7b. Submit validator registration tx (BLS Proof-of-Possession required)
./target/release/zbx-node validator register \
    --keystore ./keystore/validator.json \
    --stake 10000ZBX \
    --rpc https://rpc-testnet.zbx.io
```

---

## Step 8 — Start the Node

### As a systemd service (recommended)

```bash
# Copy the pre-built service file
sudo cp deploy/systemd/zbx-testnet.service /etc/systemd/system/

# Edit service to set your paths and env vars
sudo nano /etc/systemd/system/zbx-testnet.service

# Enable and start
sudo systemctl daemon-reload
sudo systemctl enable zbx-testnet
sudo systemctl start zbx-testnet

# Watch logs
sudo journalctl -fu zbx-testnet
```

### Manual start (for testing)

```bash
export ZBX_CHAIN_ENV=testnet
export ZBX_KZG_ALLOW_DEVNET_TAU=1
export ZBX_AI_ALLOW_STUBS=1
export RUST_LOG=info

./target/release/zbx-node start \
    --config config/testnet.toml \
    --home /var/lib/zbx/testnet
```

---

## Step 9 — Verify Node Health

```bash
# Check sync status
curl -s http://localhost:18545 -X POST \
    -H "Content-Type: application/json" \
    -d '{"jsonrpc":"2.0","method":"eth_syncing","params":[],"id":1}'
# When synced: {"result": false}

# Check block number
curl -s http://localhost:18545 -X POST \
    -H "Content-Type: application/json" \
    -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}'

# Check peer count
curl -s http://localhost:18545 -X POST \
    -H "Content-Type: application/json" \
    -d '{"jsonrpc":"2.0","method":"net_peerCount","params":[],"id":1}'
# Should be > 0

# Check validator status
curl -s http://localhost:18545 -X POST \
    -H "Content-Type: application/json" \
    -d '{"jsonrpc":"2.0","method":"zbx_validatorStatus","params":["0xYOUR_ADDRESS"],"id":1}'
```

---

## Step 10 — Monitor

Prometheus metrics are exposed at `http://localhost:9001/metrics`.

Key metrics to watch:

| Metric | Healthy Range | Alert If |
|---|---|---|
| `zbx_consensus_height` | increasing | stuck > 30s |
| `zbx_consensus_round` | 0 or 1 | > 5 sustained |
| `zbx_mempool_pending_txs` | 0–5000 | > 4000 |
| `zbx_oracle_last_update_age_secs` | < 60 | > 300 |
| `zbx_peer_count` | > 2 | 0 |
| `zbx_da_blob_commit_duration_ms` | < 200 | > 2000 |

Import `monitoring/grafana/zbx_dashboard.json` into Grafana.

---

## Expected Log Output on Healthy Start

```
INFO zbx_node::genesis   — testnet genesis loaded (chain_id=8990)
INFO zbx_node::genesis   — genesis hash pinned ✓ (0xabc123...)
WARN zbx_da::commitment  — DEVNET placeholder (G₂_τ = G₂, τ=1). Do not use on mainnet.
WARN zbx_light           — GENESIS_CHECKPOINT placeholder hash in use — production deployments must pin
INFO zbx_network         — Listening on /ip4/0.0.0.0/tcp/30304
INFO zbx_network         — Discovered 3 peers via Kademlia
INFO zbx_consensus       — HotStuff-2 engine started (epoch=0, round=0)
INFO zbx_consensus       — Elected proposer for height=1 via VRF
INFO zbx_node::block_producer — Block 1 finalized (txs=0, state_root=0x..., time=120ms)
```

The two `WARN` lines (DA devnet tau + light client genesis) are **expected and correct**.

---

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `PinError::Sentinel` on startup | Genesis hash not pinned | Complete Step 3 |
| `DaError::NotImplemented` or KZG panic | KZG env var not set | Set `ZBX_KZG_ALLOW_DEVNET_TAU=1` |
| `OPERATOR-05: placeholder address` | Genesis has zero addresses | Use `zbx-keygen generate` for validator addresses |
| Peer count = 0 | Bootstrap peers unreachable | Check firewall: port 30304 open |
| Oracle INR feed errors | Missing API key | Set `ORACLE_AI_API_KEY` or ignore (other feeds work) |
| AI precompile fails | Missing stub flag | Set `ZBX_AI_ALLOW_STUBS=1` |
| `chain_id mismatch` | Wrong binary for network | Rebuild with `cargo build --release -p zbx-node` |
| `genesis hash mismatch` | Binary compiled with different genesis | Re-pin genesis hash (Step 3) and rebuild |

---

## Differences from Mainnet Launch

| Step | Testnet | Mainnet |
|---|---|---|
| KZG | `ZBX_KZG_ALLOW_DEVNET_TAU=1` | Real Powers of Tau ceremony file |
| AI weights | `ZBX_AI_ALLOW_STUBS=1` | Real trained weight files via DA |
| Validator addresses | Can use test addresses | Must be real secp256k1 keypairs |
| Genesis placeholder check | Skipped (chain_id ≠ 8989) | Enforced (panics on placeholder) |
| CORS | `["*"]` open | Restricted allowlist |
| Solidity audit | Not required | External audit required before deploy |

---

## Related Docs

- [`docs/VALIDATOR_GUIDE.md`](VALIDATOR_GUIDE.md) — Full validator operations guide
- [`docs/TESTNET_AUDIT_2026-06-29.md`](TESTNET_AUDIT_2026-06-29.md) — Code-verified audit
- [`docs/TESTNET-VS-MAINNET-FEATURES.md`](TESTNET-VS-MAINNET-FEATURES.md) — Feature matrix
- [`docs/CONFIGURATION.md`](CONFIGURATION.md) — All config options
- [`docs/runbooks/VALIDATOR-ONBOARDING.md`](runbooks/VALIDATOR-ONBOARDING.md) — Onboarding steps
- [`config/testnet.toml`](../config/testnet.toml) — Reference testnet config
