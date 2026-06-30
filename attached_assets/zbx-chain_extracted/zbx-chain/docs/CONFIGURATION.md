# Full Configuration Reference

All configuration is loaded from a TOML file passed with `--config`.
The actual `NodeConfig` struct is defined in `node/src/config.rs`.

## CLI Flags (`zbx-node`)

| Flag | Default | Description |
|---|---|---|
| `--network mainnet\|testnet` | `mainnet` | Network preset (chain_id 8989 or 8990) |
| `--config <path>` | (none) | Path to TOML config (overrides preset) |
| `--data-dir <dir>` | (from config) | Override data directory |
| `--rpc-port <port>` | (from config) | Override RPC HTTP port |
| `--bind-addr <ip>` | (from config) | Override RPC bind address |
| `--p2p-port <port>` | (from config) | Override P2P listen port |
| `--validator` | false | Enable validator mode (requires `VALIDATOR_KEY` env) |
| `--log-level` | `info` | error / warn / info / debug / trace |
| `--print-genesis` | false | Print genesis info and exit |
| `--allow-chain-mismatch` | false | **DANGEROUS** — bypass genesis fail-fast (ops recovery only) |

---

## `[chain]`

| Key | Type | Default | Description |
|---|---|---|---|
| `chain_id` | u64 | 8989 | Network chain ID (mainnet 8989, testnet 8990) |
| `genesis_file` | path | `"genesis.json"` | Path to genesis JSON — loaded on first boot |
| `is_validator` | bool | false | Enable HotStuff-BFT consensus participation |
| `validator_key` | string | `""` | BLS private key hex (prefer `VALIDATOR_KEY` env var) |

### `[[chain.extra_validators]]`

Repeat for each other validator in the set (so this node can verify their BLS votes):

```toml
[[chain.extra_validators]]
address    = "0xValidatorEVMAddress"   # 20-byte hex
bls_pubkey = "0xBLS_G1_PubKey_hex"    # 48-byte compressed G1 point
```

---

## `[storage]`

| Key | Type | Default | Description |
|---|---|---|---|
| `data_dir` | path | `/var/lib/zbx` | RocksDB data directory |
| `max_open_files` | i32 | 4096 | RocksDB max open file descriptors |
| `cache_size_mb` | u64 | 1024 | RocksDB block cache (MB) |

---

## `[network]`

| Key | Type | Default | Description |
|---|---|---|---|
| `listen_addr` | string | `"0.0.0.0"` | TCP bind address |
| `listen_port` | u16 | 30303 | TCP listen port (mainnet 30303, testnet 30304) |
| `max_peers` | usize | 50 | Max connected peers |
| `bootnodes` | [string] | `[]` | Static bootnode enode URIs (`"enode://<ip>:<port>"`) |
| `nat` | string (optional) | `"any"` | NAT traversal mode (`"any"`, `"none"`, `"upnp"`) |

---

## `[consensus]`

| Key | Type | Default | Description |
|---|---|---|---|
| `block_time_ms` | u64 | 5000 | Target block time (milliseconds) |
| `max_block_gas` | u64 | 30,000,000 | Max gas per block |
| `mempool_max_pending` | usize | 5000 | Max pending txs in mempool |
| `mempool_max_queued` | usize | 2000 | Max queued txs in mempool |

---

## `[rpc]`

| Key | Type | Default | Description |
|---|---|---|---|
| `http_enabled` | bool | true | Enable HTTP JSON-RPC server |
| `http_port` | u16 | 8545 | HTTP listen port (mainnet 8545, testnet 18545) |
| `ws_enabled` | bool | false | Enable WebSocket JSON-RPC server |
| `ws_port` | u16 | 8546 | WebSocket listen port |
| `bind_addr` | string | `"127.0.0.1"` | Bind address (use `127.0.0.1` behind nginx/TLS, `0.0.0.0` for direct) |
| `cors_origins` | [string] | `["*"]` | CORS allowed origins |
| `rate_limit_rpm` | u32 | 600 | Requests per minute per IP (0 = unlimited) |

---

## `[metrics]`

| Key | Type | Default | Description |
|---|---|---|---|
| `enabled` | bool | true | Enable Prometheus `/metrics` endpoint |
| `port` | u16 | 9001 | Metrics listen port |

---

## Complete Testnet Config Example

```toml
# node/configs/testnet.toml
[chain]
chain_id      = 8990
genesis_file  = "/etc/zbx/genesis.testnet.json"
is_validator  = true
validator_key = ""   # set VALIDATOR_KEY env var instead

[[chain.extra_validators]]
address    = "0xA000000000000000000000000000000000000002"
bls_pubkey = "0x97f1d3a73197d7942695638c4fa9ac0fc3688c4f9774b905a14e3a3f171bac586c55e83ff97a1aeffb3af00adb22c6bb"

[storage]
data_dir       = "/var/lib/zbx-testnet"
max_open_files = 4096
cache_size_mb  = 1024

[network]
listen_addr = "0.0.0.0"
listen_port = 30304
max_peers   = 50
bootnodes   = ["enode://93.127.213.192:30304"]
nat         = "any"

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

## Environment Variables

| Variable | Description |
|---|---|
| `VALIDATOR_KEY` | BLS private key hex (`0x...32 bytes`) — preferred over `chain.validator_key` in TOML |
| `RUST_LOG` | Log filter (e.g. `zbx_node=debug,zbx_consensus=trace`) — overrides `--log-level` |

---

## Genesis JSON Format

```json
{
  "chain_id": 8990,
  "timestamp": 1735689600,
  "gas_limit": 30000000,
  "base_fee": 1000000000,
  "extra_data": "Zebvix Testnet Genesis v1",
  "validators": [
    "0xA000000000000000000000000000000000000001"
  ],
  "alloc": [
    {
      "address": "0xA000000000000000000000000000000000000001",
      "balance": "10000000000000000000000",
      "nonce": 0
    }
  ]
}
```

> `balance` **must be a quoted decimal string** for values > 18 ZBX (> u64::MAX in Wei).
> The `balance_serde` deserializer in `genesis.rs` accepts both strings and u64-range integers.
