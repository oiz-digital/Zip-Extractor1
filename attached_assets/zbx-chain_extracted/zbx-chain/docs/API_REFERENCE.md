# Zebvix Chain RPC API Reference

**Base URL**: `http://localhost:8545` (mainnet) / `http://localhost:18545` (testnet)  
**Protocol**: JSON-RPC 2.0  
**Content-Type**: `application/json`

All requests use the JSON-RPC 2.0 envelope:
```json
{"jsonrpc": "2.0", "id": 1, "method": "<method>", "params": [...]}
```

---

## Ethereum-Compatible Methods (`eth_*`)

### eth_chainId
Returns the chain ID.
```json
{"method": "eth_chainId"}
→ "0x231D"  (8989 decimal, mainnet)
→ "0x232E"  (8990 decimal, testnet)
```

### eth_blockNumber
Returns the current highest block number.
```json
{"method": "eth_blockNumber"}
→ "0x4A2BC0"
```

### eth_getBalance
```json
{"method": "eth_getBalance", "params": ["0xAddress", "latest"]}
→ "0xDE0B6B3A7640000"  (1 ZBX = 10^18 Wei)
```

### eth_getTransactionCount
```json
{"method": "eth_getTransactionCount", "params": ["0xAddress", "latest"]}
→ "0x5"  (nonce = 5)
```

### eth_getCode
```json
{"method": "eth_getCode", "params": ["0xContractAddress", "latest"]}
→ "0x608060..."  (contract bytecode, or "0x" for EOA)
```

### eth_getStorageAt
```json
{"method": "eth_getStorageAt", "params": ["0xContract", "0x0", "latest"]}
→ "0x000...0001"  (storage slot value)
```

### eth_call
Dry-run contract call (no state change).
```json
{"method": "eth_call", "params": [{"to": "0xContract", "data": "0xCalldata"}, "latest"]}
→ "0xReturnData"
```

### eth_estimateGas
```json
{"method": "eth_estimateGas", "params": [{"to": "0xRecipient", "value": "0xDE0B6B3A7640000"}]}
→ "0x5208"  (21,000 = simple transfer)
```

### eth_gasPrice
EIP-1559 oracle — median `base_fee` of last 10 blocks + 10% buffer + 1 gwei priority tip.
```json
{"method": "eth_gasPrice"}
→ "0x3B9ACA00"  (1 gwei on cold start)
```

### eth_feeHistory
```json
{"method": "eth_feeHistory", "params": [10, "latest", [25, 75]]}
→ { "baseFeePerGas": [...], "gasUsedRatio": [...], "reward": [...] }
```

### eth_sendRawTransaction
Submit a signed RLP-encoded transaction. On success, the tx is added to mempool and relayed to all P2P peers.
```json
{"method": "eth_sendRawTransaction", "params": ["0xSignedTxRLP"]}
→ "0xTxHash"
```

### eth_getBlockByNumber
```json
{"method": "eth_getBlockByNumber", "params": ["latest", true]}
→ { "number": "0x...", "hash": "0x...", "transactions": [...], ... }
```
Second param: `true` = full tx objects, `false` = tx hashes only.

### eth_getBlockByHash
```json
{"method": "eth_getBlockByHash", "params": ["0xBlockHash", false]}
```

### eth_getBlockTransactionCountByNumber
```json
{"method": "eth_getBlockTransactionCountByNumber", "params": ["latest"]}
→ "0x1F"  (31 txs)
```

### eth_getBlockTransactionCountByHash
```json
{"method": "eth_getBlockTransactionCountByHash", "params": ["0xBlockHash"]}
```

### eth_getTransactionByHash
```json
{"method": "eth_getTransactionByHash", "params": ["0xTxHash"]}
→ { "hash": "0x...", "from": "0x...", "to": "0x...", "value": "0x...", ... }
```

### eth_getTransactionReceipt
```json
{"method": "eth_getTransactionReceipt", "params": ["0xTxHash"]}
→ { "status": "0x1", "gasUsed": "0x5208", "logs": [...], ... }
```

### eth_getLogs
EIP-3068 log filter. Capped at 2,000 blocks per request (DoS protection).
```json
{
  "method": "eth_getLogs",
  "params": [{
    "fromBlock": "0x0",
    "toBlock":   "latest",
    "address":   "0xContractAddress",
    "topics":    ["0xEventTopic0", null, "0xEventTopic2"]
  }]
}
→ [{ "address": "0x...", "topics": [...], "data": "0x...", "blockNumber": "0x..." }]
```

### eth_syncing
```json
{"method": "eth_syncing"}
→ false                      (synced)
→ { "currentBlock": "0x...", "highestBlock": "0x..." }   (syncing)
```

### web3_sha3
Keccak-256 hash.
```json
{"method": "web3_sha3", "params": ["0x68656c6c6f"]}
→ "0x1c8aff950685c2ed4bc3174f3472287b56d9517b9c948127319a09a7a36deac8"
```

---

## Network Methods (`net_*`)

| Method | Returns |
|---|---|
| `net_version` | `"8989"` (mainnet) or `"8990"` (testnet) |
| `net_listening` | `true` |
| `net_peerCount` | `"0x10"` (hex peer count from P2P layer) |

---

## ZBX-Native Methods (`zbx_*`)

### zbx_getChainInfo
Returns chain metadata.
```json
{"method": "zbx_getChainInfo"}
→ {
    "chainId": 8989,
    "chainName": "Zebvix Chain",
    "nativeToken": "ZBX",
    "blockTime": 5,
    "consensus": "HotStuff-BFT",
    "version": "0.2.0"
  }
```

### zbx_getValidatorSet
Returns current active validator set with stake and BFT quorum.
```json
{"method": "zbx_getValidatorSet"}
→ {
    "epoch": 12,
    "validators": [
      { "address": "0x...", "blsPubkey": "0x...", "stake": "10000000000000000000000", "active": true }
    ],
    "totalStake": "30000000000000000000000",
    "quorum": 2
  }
```

### zbx_getStakingInfo
Returns staking info for a specific validator or delegator address.
```json
{"method": "zbx_getStakingInfo", "params": ["0xAddress"]}
→ {
    "selfStake":     "10000000000000000000000",
    "delegatedStake": "5000000000000000000000",
    "commissionBps": 500,
    "pendingRewards": "100000000000000000",
    "status": "active",
    "inActiveSet": true
  }
```

### zbx_getBridgeInfo
```json
{"method": "zbx_getBridgeInfo"}
→ { "supportedChains": ["ethereum","bsc","polygon"], "bridgeVault": "0x...", "multisig": "0x..." }
```

### zbx_getBlockReward
```json
{"method": "zbx_getBlockReward", "params": ["0xBlockHash"]}
→ { "baseReward": "3000000000000000000", "priorityFees": "...", "mevShare": "..." }
```

### zbx_getEpochInfo
```json
{"method": "zbx_getEpochInfo"}
→ { "epoch": 12, "startBlock": "0x...", "blocksPerEpoch": 43200, "nextRotation": "0x..." }
```

### zbx_proposeGovernance / zbx_getGovernanceProposal
Submit or read on-chain governance proposals.

---

## WebSocket Subscriptions (`eth_subscribe`)

Connect to `ws://localhost:8546` (or `ws://localhost:18546` for testnet).

```javascript
// New blocks
ws.send(JSON.stringify({
  jsonrpc: "2.0", method: "eth_subscribe",
  params: ["newHeads"], id: 1
}));
// → {"jsonrpc":"2.0","method":"eth_subscription","params":{"subscription":"0x1","result":{...block...}}}

// Pending transactions (tx hash on mempool accept)
ws.send(JSON.stringify({
  jsonrpc: "2.0", method: "eth_subscribe",
  params: ["newPendingTransactions"], id: 2
}));
// → {"params":{"result":"0xTxHash",...}}

// Filtered logs
ws.send(JSON.stringify({
  jsonrpc: "2.0", method: "eth_subscribe",
  params: ["logs", {"address": "0xContract", "topics": ["0xEventTopic"]}], id: 3
}));

// Unsubscribe
ws.send(JSON.stringify({
  jsonrpc: "2.0", method: "eth_unsubscribe",
  params: ["0x1"], id: 4
}));
```

---

## Rate Limiting

| Network | Default limit |
|---|---|
| Mainnet | 600 req/min per IP |
| Testnet | 1200 req/min per IP |

Override with `rpc.rate_limit_rpm = 0` in config to disable (private nodes only).

---

## Quick Reference curl Examples

```bash
RPC=http://localhost:18545

# Block number
curl -s -X POST $RPC -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}'

# Balance
curl -s -X POST $RPC -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"eth_getBalance","params":["0xAddress","latest"],"id":1}'

# Validator set
curl -s -X POST $RPC -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"zbx_getValidatorSet","params":[],"id":1}'
```
