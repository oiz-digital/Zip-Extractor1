# Zebvix Chain JSON-RPC API Reference

The Zebvix Chain node exposes a JSON-RPC 2.0 API compatible with the Ethereum
JSON-RPC specification (EIP-1474) plus ZBX-specific extensions.

**Version**: 0.2.0  
**Implemented in**: `crates/zbx-rpc/src/eth_api.rs` + `zbx_api.rs`

## Transport

| Transport | Default (mainnet) | Default (testnet) |
|---|---|---|
| HTTP | `http://localhost:8545` | `http://localhost:18545` |
| WebSocket | `ws://localhost:8546` | `ws://localhost:18546` |

All requests use the JSON-RPC 2.0 envelope:
```json
{ "jsonrpc": "2.0", "id": 1, "method": "<method>", "params": [...] }
```

---

## Implemented `eth_*` Methods

| Method | Description |
|---|---|
| `eth_chainId` | Chain ID (`0x231D` = 8989 mainnet, `0x232E` = 8990 testnet) |
| `eth_blockNumber` | Highest block number |
| `eth_getBalance` | Account ZBX balance in Wei |
| `eth_getTransactionCount` | Account nonce |
| `eth_getCode` | Contract bytecode |
| `eth_getStorageAt` | Contract storage slot value |
| `eth_call` | Dry-run contract call (no state change) |
| `eth_estimateGas` | Gas estimation |
| `eth_gasPrice` | EIP-1559 oracle: median base_fee (last 10 blocks) + 10% buffer + 1 gwei tip |
| `eth_feeHistory` | EIP-1559 fee history |
| `eth_sendRawTransaction` | Submit signed tx → mempool → P2P relay to all peers |
| `eth_getBlockByNumber` | Block by number or tag (`latest`, `earliest`, `pending`) |
| `eth_getBlockByHash` | Block by hash |
| `eth_getBlockTransactionCountByNumber` | Tx count in block (by number) |
| `eth_getBlockTransactionCountByHash` | Tx count in block (by hash) |
| `eth_getTransactionByHash` | Transaction detail |
| `eth_getTransactionReceipt` | Receipt (logs, gas used, status) |
| `eth_getLogs` | EIP-3068 log filter — fromBlock, toBlock, address, topics (capped 2,000 blocks) |
| `eth_syncing` | Sync status (false if synced, object if syncing) |
| `web3_sha3` | Keccak-256 hash |

---

## Implemented `net_*` Methods

| Method | Returns |
|---|---|
| `net_version` | `"8989"` or `"8990"` |
| `net_listening` | `true` |
| `net_peerCount` | Hex peer count from P2P layer |

---

## Implemented `zbx_*` Methods

| Method | Description |
|---|---|
| `zbx_getChainInfo` | Chain ID, name, genesis hash, block time, version |
| `zbx_getValidatorSet` | Epoch, validators, total stake, BFT quorum |
| `zbx_getStakingInfo` | Per-address: selfStake, delegated, commission, rewards, status |
| `zbx_getBridgeInfo` | Bridge contract addresses, supported chains |
| `zbx_getBlockReward` | Block reward breakdown |
| `zbx_getEpochInfo` | Current epoch, duration, next rotation block |
| `zbx_proposeGovernance` | Submit governance proposal |
| `zbx_getGovernanceProposal` | Read governance proposal |

---

## WebSocket Subscriptions (`eth_subscribe` / `eth_unsubscribe`)

Requires WebSocket connection (`ws_enabled = true` in config).

| Subscription | Push event |
|---|---|
| `newHeads` | Full block header on every new sealed block |
| `newPendingTransactions` | Tx hash on every mempool accept |
| `logs` | Filtered logs matching address/topics filter |

```bash
# Example: subscribe to new blocks via wscat
wscat -c ws://localhost:18546
> {"jsonrpc":"2.0","method":"eth_subscribe","params":["newHeads"],"id":1}
```

---

## eth_sendRawTransaction — TX Relay Flow

```
eth_sendRawTransaction(signed_rlp)
  │
  ├── Decode RLP + verify signature
  ├── Mempool.add_transaction(tx)      ← validate nonce, balance, gas
  │     OK
  ├── broadcast::Sender.send(tx)       ← in-process channel
  │
  └── NetworkServer relay task
        └── Message::Transaction(tx) → all connected TCP peers
```

---

## eth_getLogs — Filter Reference

```json
{
  "fromBlock": "0x100",           // hex block number, "latest", "earliest"
  "toBlock":   "latest",
  "address":   "0xContract",     // optional: filter by contract
  "topics": [                     // optional: filter by topics
    "0xEventSig",                 // topic[0] — event signature
    null,                         // wildcard (match any)
    "0xIndexedParam"              // topic[2] — indexed parameter
  ]
}
```

Max range: 2,000 blocks per request.

---

## eth_gasPrice — Oracle Logic

```rust
// Implemented in crates/zbx-rpc/src/eth_api.rs
let last_10_blocks = db.get_recent_blocks(10);
let base_fees: Vec<u64> = last_10_blocks.map(|b| b.header.base_fee_per_gas);
let median = sorted_median(base_fees);
let suggested = median * 11 / 10 + 1_000_000_000;  // +10% buffer + 1 gwei priority tip
```

Falls back to `1_000_000_000` (1 gwei) on cold start (no blocks yet).

---

## Rate Limiting

| Config | Default | Description |
|---|---|---|
| `rpc.rate_limit_rpm` | 600 (mainnet), 1200 (testnet) | Max requests per minute per IP |

Set to `0` to disable for private/local nodes.

---

## Chain ID Reference

```bash
# Mainnet
curl -s -X POST http://localhost:8545 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":1}'
# → {"result":"0x231d",...}   (8989 decimal)

# Testnet
curl -s -X POST http://localhost:18545 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":1}'
# → {"result":"0x231e",...}   (8990 decimal)
```
