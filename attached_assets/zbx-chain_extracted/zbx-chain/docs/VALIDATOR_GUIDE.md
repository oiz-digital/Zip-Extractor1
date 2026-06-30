# Validator Guide — Zebvix Chain

**Version**: 0.2.0  
**Min self-stake**: 100 ZBX  
**Chain IDs**: 8989 (mainnet) / 8990 (testnet)

---

## Requirements

| Resource | Minimum | Recommended |
|---|---|---|
| CPU | 4 cores | 16 cores |
| RAM | 8 GB | 64 GB |
| Storage | 500 GB SSD | 2 TB NVMe |
| Network | 100 Mbps | 1 Gbps |
| Uptime | 95% | 99.9% |
| ZBX Stake | 100 ZBX (min self-stake) | 10,000+ ZBX |

---

## Setup

### 1. Build the Node Binaries

```bash
git clone https://github.com/zebvix/zbx-chain
cd zbx-chain

# Requires libclang (for RocksDB)
export LIBCLANG_PATH=/usr/lib/llvm-15/lib

# Build both binaries
cargo build --release --bin zbx-node --bin zbx-keygen
```

Or use Docker:

```bash
docker build -t zbx/zbx-node:0.2.0 -f zbx-chain/docker/Dockerfile .
```

---

### 2. Generate Validator Keys

Use the `zbx-keygen` tool (included in the binary release):

```bash
# Generate one keypair
./zbx-keygen --count 1 --output text
```

Example output:
```
=== Validator Keypair #1 ===

  EVM Address   : 0xAbCd...1234
  BLS PubKey    : 0xa3f...  (48 bytes — goes in genesis validators[])

  !! KEEP THESE PRIVATE !!
  BLS PrivKey   : 0x7b2...  (32 bytes — set as VALIDATOR_KEY env var)
  Node PrivKey  : 0xef1...  (32 bytes — secp256k1, for P2P identity)

  --- Genesis alloc snippet ---
  {"address": "0xAbCd...1234", "balance": "10000000000000000000000", "nonce": 0},

  --- testnet-genesis.json validators[] snippet ---
  "validators": [
    "0xAbCd...1234"
  ]
```

> **Security**: Private keys are printed to stdout only. Never commit them or put them in config files. Use the VALIDATOR_KEY environment variable.

For a multi-validator testnet (generate one keypair per VPS):
```bash
./zbx-keygen --count 3 --output json > validators.json
```

---

### 3. Configure the Node

The production config format for `node/configs/testnet.toml` (matches `NodeConfig` struct):

```toml
[chain]
chain_id      = 8990
genesis_file  = "/etc/zbx/genesis.testnet.json"
is_validator  = true
validator_key = ""   # leave empty; set VALIDATOR_KEY env var instead

# List other validators' BLS pubkeys so this node can verify their votes
[[chain.extra_validators]]
address    = "0xOtherValidator1"
bls_pubkey = "0x..."

[[chain.extra_validators]]
address    = "0xOtherValidator2"
bls_pubkey = "0x..."

[storage]
data_dir       = "/var/lib/zbx-testnet"
max_open_files = 4096
cache_size_mb  = 1024

[network]
listen_addr = "0.0.0.0"
listen_port = 30304
max_peers   = 50
bootnodes   = [
  "enode://93.127.213.192:30304",
]
nat = "any"

[consensus]
block_time_ms        = 5000
max_block_gas        = 30000000
mempool_max_pending  = 5000
mempool_max_queued   = 2000

[rpc]
http_enabled    = true
http_port       = 18545
ws_enabled      = false
ws_port         = 18546
bind_addr       = "127.0.0.1"
cors_origins    = ["*"]
rate_limit_rpm  = 1200

[metrics]
enabled = true
port    = 9001
```

---

### 4. Set the Validator Key

```bash
# On the VPS — never hardcode in config file
export VALIDATOR_KEY=0x<your_bls_privkey_from_zbx-keygen>
```

For systemd service:
```ini
[Service]
Environment="VALIDATOR_KEY=0x..."
ExecStart=/usr/local/bin/zbx-node --network testnet --config /etc/zbx/testnet.toml --validator
```

---

### 5. Update Genesis (if first-time testnet launch)

Add your EVM address to `config/testnet-genesis.json`:

```json
{
  "chain_id": 8990,
  "timestamp": 1735689600,
  "gas_limit": 30000000,
  "base_fee": 1000000000,
  "extra_data": "Zebvix Testnet Genesis v1",
  "validators": [
    "0x<your_evm_address>",
    "0x<validator2_evm_address>",
    "0x<validator3_evm_address>"
  ],
  "alloc": [
    { "address": "0x<your_evm_address>", "balance": "10000000000000000000000", "nonce": 0 },
    { "address": "0x<faucet_address>",   "balance": "5000000000000000000000000", "nonce": 0 }
  ]
}
```

> Balances must be **quoted decimal strings** (e.g. `"10000000000000000000000"` = 10,000 ZBX). JSON integers larger than 2^53 lose precision.

---

### 6. Start the Node

```bash
zbx-node \
  --network testnet \
  --config /etc/zbx/testnet.toml \
  --validator \
  --log-level info
```

---

## Monitoring

```bash
# Sync status
curl -s -X POST http://127.0.0.1:18545 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"eth_syncing","params":[],"id":1}'

# Peer count
curl -s -X POST http://127.0.0.1:18545 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"net_peerCount","params":[],"id":1}'

# Current block
curl -s -X POST http://127.0.0.1:18545 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}'

# Prometheus metrics
curl http://localhost:9001/metrics
```

---

## Slashing Conditions

| Violation | Penalty |
|---|---|
| Double-signing (equivocation) | 5% stake burn + permanent jail |
| Downtime > 5% blocks in epoch | 0.01% stake |
| Invalid state proof | 10% stake |

Slashed funds: 50% burned, 50% to reporter's reward.

---

## Rewards

- Block rewards: 3 ZBX/block (Era 0, halves every 25M blocks)
- Priority fees: proportional to stake
- MEV redistribution: ~0.5–2% additional APR
- Approximate APY: 12–18% (varies with total staked ZBX and participation)
