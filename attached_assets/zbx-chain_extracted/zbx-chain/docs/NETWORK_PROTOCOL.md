# Zebvix Network Protocol

Zebvix Chain uses a custom application-layer protocol over raw TCP.

---

## Transport

- **Primary**: TCP (mainnet port 30303, testnet port 30304)
- **Encryption**: Noise XX handshake on every connection (authenticated + encrypted)
- **Framing**: 4-byte big-endian length prefix + JSON payload

```
| length (4 bytes, BE) | JSON payload (variable) |
```

---

## Node Identity

Each node has a unique **identity** derived from its secp256k1 private key.
The EVM address (`keccak256(pubkey)[12..]`) serves as the node identifier.

Bootnode addresses are expressed as enode URIs (IP only — no pubkey required for initial dial):
```
enode://<ip>:<port>
```

---

## Connection Lifecycle

```
Dialer                              Listener
  │                                    │
  │─── TCP connect ───────────────────▶│
  │◀── Noise XX handshake ────────────▶│
  │                                    │
  │─── Status JSON ───────────────────▶│  (chain_id, best_height, genesis_hash)
  │◀── Status JSON ────────────────────│
  │                                    │
  │  height check:                     │
  │  if peer.height > self.height:     │
  │    send GetBlockRange              │
  │                                    │
  │─── FindPeers ─────────────────────▶│  (trigger peer discovery)
  │◀── Peers(Vec<String>) ─────────────│  (list of known peer addresses)
  │                                    │
  │  [ normal message exchange ]       │
```

---

## Message Types

All messages are JSON-encoded. Defined in `crates/zbx-network/src/messages.rs`.

| Type Code | Message | Direction | Purpose |
|---|---|---|---|
| 0x01 | `Ping` | both | Keep-alive heartbeat |
| 0x01 | `Pong` | both | Keep-alive response |
| 0x10 | `GetBlockRange { from, to }` | outbound | Request block sync range |
| 0x11 | `Block(Block)` | inbound | Single block delivery |
| 0x12 | `Blocks(Vec<Block>)` | inbound | Batch block delivery |
| 0x13 | `GetBlockByHash(H256)` | outbound | Fetch specific block |
| 0x20 | `Transaction(SignedTransaction)` | both | Single TX relay after mempool accept |
| 0x21 | `Transactions(Vec<SignedTransaction>)` | both | Batch TX relay |
| 0x40 | `Vote(Vote)` | both | HotStuff-BFT vote propagation |
| 0x04 | `FindPeers { target }` | outbound | Peer discovery request |
| 0x05 | `Peers(Vec<String>)` | inbound | Peer address list response |

---

## Peer Discovery

1. On every new connection, node sends `FindPeers` to request known peers
2. Peer responds with `Peers(Vec<addr>)` — list of known peer multiaddrs
3. Node dials each new (unknown) peer independently via `tokio::spawn(dial_peer(addr))`
4. Static bootnodes configured in `[network] bootnodes` in TOML config

### Bootnode Reconnect

If a bootnode disconnects, the node reconnects with exponential backoff:
- Initial delay: 5 seconds
- Max delay: 120 seconds (cap)
- Loops forever — bootnode is always re-dialled

---

## Block Sync

```
New peer connects (peer.height > self.height)
  │
  ├── Send GetBlockRange { from: self.height + 1, to: peer.height }
  │
  ├── Receive Blocks(Vec<Block>)
  │     └── For each block: execute_and_commit() → StateDB
  │
  └── Request next range (continuous pipeline until synced)
```

---

## TX Relay Flow

```
User calls eth_sendRawTransaction
  │
  ▼
Mempool.add_transaction(tx)          ← validates signature, nonce, balance
  │  OK
  ▼
broadcast::Sender<SignedTransaction>  ← in-process channel
  │
  ▼
NetworkServer relay task
  │
  ├── sends Message::Transaction(tx) to all connected peers
  └── peers add to their own mempools
```

---

## Multi-Validator Config (TOML)

Other validators' BLS pubkeys are specified in the node config so this validator can verify their votes:

```toml
[chain]
chain_id    = 8990
is_validator = true

[[chain.extra_validators]]
address    = "0xValidator2EvmAddress"
bls_pubkey = "0x<48-byte-BLS-G1-pubkey-hex>"

[[chain.extra_validators]]
address    = "0xValidator3EvmAddress"
bls_pubkey = "0x<48-byte-BLS-G1-pubkey-hex>"
```

Generate keypairs with `zbx-keygen`:
```bash
zbx-keygen --count 3 --output text
```

---

## Rate Limiting and Limits

| Parameter | Value |
|---|---|
| Max peers | 50 (testnet), configurable |
| Max message (implicit) | JSON framing — up to 64 MB |
| Reconnect cap | 120 seconds |
| Reconnect initial | 5 seconds |
| Rate limit (RPC) | 600 req/min (mainnet), 1200 req/min (testnet) |
