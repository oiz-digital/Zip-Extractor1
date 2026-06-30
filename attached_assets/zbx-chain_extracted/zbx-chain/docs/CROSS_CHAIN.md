# Cross-Chain Guide

**Last updated**: 2026-05-03 (Session 21 — Bridge multi-token support)  
**Status**: CURRENT

ZBX Chain has two cross-chain systems — use the right one for your use case:

| | Bridge (`zbx-bridge`) | XCL (`zbx-xcl`) |
|---|---|---|
| **Purpose** | Multi-token transfer to ETH/BSC/Polygon | Trustless tokens + **arbitrary messages** |
| **Trust model** | 3-of-5 multisig | **Zero trust — cryptographic proof** |
| **Relayers** | Permissioned (5 nodes) | **Permissionless — anyone can relay** |
| **What it moves** | ZBX, ZUSD, ZBXBTC, ZBXETH, ZBXUSDC | Native tokens + **any byte payload** |
| **Verification** | Off-chain multisig + Merkle receipt proof | **BLS12-381 pairing + MPT Merkle proof** |
| **Protocols** | Lock-and-Mint / Burn-and-Mint | FT-1 (tokens) + **MSG-1 (messages)** |
| **Status** | Testnet (ETH/BSC/Polygon) | Production |

---

## Part 1 — Bridge (`zbx-bridge`)

Use the bridge for connectivity to Ethereum, BSC, and Polygon.

### Supported Networks

| Network | Chain ID | Confirmations | Wait time |
|---------|----------|---------------|-----------|
| **Ethereum Mainnet** | 1 | 12 blocks | ~2.5 min |
| **BNB Chain (BSC)** | 56 | 20 blocks | ~60 sec |
| **Polygon** | 137 | 128 blocks | ~4 min |

### Supported Tokens

ZBX Chain has two native tokens — both bridgeable via Lock-and-Mint:

| Token | Max per TX | Daily Limit | Model |
|-------|-----------|-------------|-------|
| **ZBX** (native protocol token) | 1,000,000 ZBX | 10,000,000 ZBX/day | Lock-and-Mint |
| **ZUSD** (USD stablecoin, native) | 5,000,000 ZUSD | 50,000,000 ZUSD/day | Lock-and-Mint |

Both tokens can pay ZBX Chain gas fees (set `gas_token` = ZBX / ZUSD in transaction).

**Bridge fee**: 0.1% (10 bps) on all tokens → protocol treasury.

### Bridging ZBX → Ethereum

```bash
# 1. Lock ZBX on ZBX Chain
zbx bridge lock \
  --token   native \
  --amount  1000 \
  --dest-chain ethereum \
  --recipient  0xYourEthAddress

# 2. Relayers observe, each signs independently
# 3. After 3-of-5 signatures + 12 ETH block confirmations:
zbx bridge claim --tx-hash 0x...
# Wrapped WZBX ERC-20 arrives in your Ethereum wallet
```

### Bridging ZUSD → BSC

```bash
zbx bridge lock \
  --token   ZUSD \
  --amount  10000 \
  --dest-chain bsc \
  --recipient  0xYourBscAddress
# ZUSD burned on ZBX Chain, ZUSD minted on BSC after 20 block confirmations
```

### Bridge Security Model

```
User locks/burns token on ZBX Chain
          │
          ▼
TokenWhitelist validates:
  ✓ token whitelisted + enabled?
  ✓ amount ≤ max_per_tx?
  ✓ daily limit not exceeded?
          │
          ▼
BridgeVault emits DepositEvent
          │
          ├─ Relayer 1 signs — confirm(id, sig, ts)
          ├─ Relayer 2 signs — confirm(id, sig, ts)
          ├─ Relayer 3 signs — confirm(id, sig, ts) → Ok(true)
          │                    [Err(InvalidSignature) if any sig is bad]
          ▼
Relayer attaches Merkle receipt proof → set_proof(proof)
          │
          ▼
execute(id, receipts_root, ts) checks:
  ✓ not expired (24h TTL)
  ✓ 3-of-5 threshold verified
  ✓ Merkle proof valid against receipts_root
          │
          ▼
BridgeAction::MintOnTarget { token, recipient, amount }
  → Execution layer mints on target chain
```

**Security guarantees**:
- Funds safe if up to 2 of 5 relayers are compromised
- Any bad/unknown/duplicate signature rejects the **entire batch** (`InvalidSignature`)
- Per-token daily limits cap maximum bridge outflow (DailyLimitTracker)
- Merkle receipt proof required — no proof, no execution
- 24-hour request TTL — stuck requests auto-expire
- Global emergency pause available (`set_paused(true)`)
- Nonce deduplication prevents duplicate-mint attacks (S11-BRIDGE-SOL-OUT1 fix)

---

## Part 2 — XCL Cross-Chain Layer (`zbx-xcl`)

XCL is the **trustless, bridge-free** interoperability layer. It uses cryptographic
proofs (BLS12-381 light clients + Merkle Patricia Trie proofs) so no relayer has
any authority — bad proofs are rejected on-chain.

XCL supports two application protocols:

### FT-1 — Fungible Token Transfer (app_id = `0x01`)

Moves ZBX or any ZBX-chain token to a foreign chain and back. Native lock/release
model keeps total supply at the 150M cap.

```rust
// Send 500 ZBX from ZBX Chain to Chain B
let ft = FtPacketData::new(
    Denom::native("ZBX"),
    500 * 1_000_000_000_000_000_000u128,  // 500 ZBX in wei
    sender_address,
    receiver_on_chain_b,
    "".to_string(),
)?;
let (packet, result) = handler.send_packet(channel_id, ft, timeout_height, 0)?;
// result.state_changes = [EscrowDeposit { from: sender, amount: 500 ZBX }]
```

**Denom namespacing** keeps supply accounting safe across hops:

```
ZBX leaves Chain 8989 via channel 0xABCD:
  Source chain: "ZBX" locked in escrow
  Dest chain:   "xcl/8989/abcd.../ZBX"  ← namespaced IOU

ZBX returns home:
  namespaced IOU burned on foreign chain
  "ZBX" released from escrow on Chain 8989
  → Total supply stays at 150M cap
```

---

### MSG-1 — Arbitrary Cross-Chain Message (app_id = `0x02`)

Send **any byte payload** to a contract on a foreign chain — ABI-encoded calldata,
JSON, a governance vote, a price update, an NFT metadata request — anything.

No tokens are escrowed. The same trustless proof machinery (BLS light client +
MPT Merkle proof) applies. The receiving chain delivers the payload to the target
contract as a low-level call.

```rust
// Call a governance contract on Chain B from Chain A
let msg = MsgPacketData::new(
    my_address,                               // sender
    governance_contract_on_chain_b,           // receiver contract
    b"executeProposal(uint256)".to_vec(),     // any calldata
)?;
let (packet, result) = handler.send_message(channel_id, msg, timeout_height, 0)?;
// result.state_changes = []  ← no escrow for MSG-1
```

**On the receiving chain** (`recv_packet`):

```
Packet arrives → handler dispatches on app_id byte:
  0x01 → FT-1: token release/credit (EscrowRelease or CreditIou)
  0x02 → MSG-1: DeliverMessage state change
             → execution layer calls receiver contract with payload
  0x?? → rejected: UnsupportedApp error
```

**MSG-1 payload limits**:
- Maximum payload size: **64 KiB** (65,535 bytes)
- Payload can be: ABI calldata, JSON, raw bytes — any format
- No special encoding required

---

### XCL Packet Lifecycle

```
Chain A                              Permissionless Relayer              Chain B
───────                              ──────────────────────              ───────
① send_packet / send_message         
  → commitment stored in MPT
  → PacketSent event                 
                                     ② reads commitment proof
                                        submits to Chain B ────────────►
                                                                         ③ recv_packet
                                                                            verify BLS QC
                                                                            verify MPT proof
                                                                            execute app logic
                                                                            write receipt + ack
                                     ④ reads ack proof ◄─────────────────
                                        submits to Chain A
④ ack_packet
  → verify ack MPT proof
  → success: clear commitment
  → error:   refund sender
```

**Timeout path** (if Chain B never receives):

```
Chain A: timeout_packet(proof_of_absence_on_chain_b)
  → verify non-inclusion MPT proof
  → refund sender (FT-1) or no-op (MSG-1)
  → PacketTimeout event
```

---

### Channel Handshake

Before packets can flow, a 4-step handshake establishes a channel:

```
Chain A                              Chain B
───────                              ───────
CHAN_OPEN_INIT  ──────────────────► CHAN_OPEN_TRY
CHAN_OPEN_ACK   ◄────────────────── CHAN_OPEN_ACK
CHAN_OPEN_CONFIRM ────────────────► OPEN ✅
```

Channel states: `Init → TryOpen → Open → Closed`

Channels support two orderings:
- **Ordered** — packets must be received in strict sequence
- **Unordered** — packets can arrive in any order (recommended for MSG-1)

---

### BLS12-381 Light Client

Each side maintains a light client tracking the counterparty's headers:

```rust
// Verify a foreign header before accepting any packet proof
client.update_header(ForeignHeader {
    number:        42_000,
    hash:          block_hash,
    state_root:    state_root,
    quorum_cert:   bls_aggregate_sig_96_bytes,   // real BLS12-381 pairing
    signer_bitmap: bitmap,                        // which validators signed
    ..
})?;
// Checks: height > latest, parent hash continuity, 2f+1 BLS QC
```

A forged header would require forging a BLS aggregate signature from 2f+1
validators — computationally infeasible under the discrete log assumption on
BLS12-381.

---

### EVM Precompile — `0x0b`

Solidity contracts can send XCL packets without any off-chain tooling:

```solidity
// ABI: xcl_send(bytes32 channel, address receiver, bytes calldata payload) 
//      returns (uint64 sequence)
(bool ok, bytes memory ret) = address(0x0b).call(
    abi.encode(channel_id, receiver, payload)
);
uint64 sequence = abi.decode(ret, (uint64));
```

---

### Code Reference

```
crates/zbx-xcl/src/
  ├─ packet.rs      — XclPacket, PacketStatus, commitment_key derivation
  ├─ channel.rs     — Channel, ChannelRegistry, 4-step handshake state machine
  ├─ client.rs      — ForeignClient: BLS12-381 QC + MPT proof verification
  ├─ commitment.rs  — keccak256 commitment/receipt/ack store
  ├─ transfer.rs    — FT-1 protocol: denom namespacing, lock/release
  ├─ message.rs     — MSG-1 protocol: arbitrary byte payload (NEW Session 20)
  ├─ handler.rs     — send_packet, send_message, recv_packet (dispatch),
  │                   ack_packet, timeout_packet
  ├─ relay.rs       — Permissionless relayer job queue
  └─ precompile.rs  — EVM precompile 0x0b
```

---

---

## Part 3 — ZBX-XCM Oracle Relay (ZEP-026, Session 40)

**Last updated**: 2026-05-05 (Session 40 — `multi_chain.rs`)

The oracle relay is a distinct cross-chain subsystem that pushes ZBX oracle price
data to 6 external EVM networks using BLS-signed relay messages (ZEP-026).

### How it differs from the Bridge and XCL

| | Bridge | XCL (zbx-xcl) | **ZBX-XCM Oracle Relay** |
|---|---|---|---|
| **Purpose** | Token transfer | Token + arbitrary messages | **Price data only** |
| **Data direction** | Bidirectional | Bidirectional | ZBX → external (push) |
| **Verification** | 3-of-5 multisig | BLS12-381 light client | **BLS oracle committee sig** |
| **Consumers** | Users | DeFi protocols | Chainlink-compatible contracts |
| **Module** | `zbx-bridge` | `zbx-xcl` | **`zbx-oracle/src/multi_chain.rs`** |

### Supported networks (8 total)

| # | Network | Chain ID | Relay type | Finality | Consumer contract |
|:--|:--------|:---------|:-----------|:---------|:------------------|
| 1 | ZBX Mainnet | **8989** | Native | Instant | `ZbxAggregatorV3` (native) |
| 2 | ZBX Testnet | **8990** | Native | Instant | `ZbxAggregatorV3` (native) |
| 3 | Ethereum | 1 | XCM push | 12 blocks | `ZbxAggregatorETH.sol` |
| 4 | BNB Smart Chain | 56 | XCM push | 15 blocks | `ZbxAggregatorBSC.sol` |
| 5 | Polygon | 137 | XCM push | 128 blocks | `ZbxAggregatorPoly.sol` |
| 6 | Arbitrum One | 42,161 | XCM push | Optimistic | `ZbxAggregatorArb.sol` |
| 7 | Optimism | 10 | XCM push | Optimistic | `ZbxAggregatorOP.sol` |
| 8 | Avalanche C-Chain | 43,114 | XCM push | Instant | `ZbxAggregatorAvax.sol` |

### `RelayMessage` structure

```rust
pub struct RelayMessage {
    pub source_chain:  u64,       // 8989 (ZBX mainnet)
    pub target_chain:  u64,       // destination chain ID
    pub feed_id:       [u8; 32],  // keccak256 of feed name
    pub price:         u128,      // price × 10^8
    pub round_id:      u64,
    pub timestamp:     u64,
    pub bls_signature: [u8; 96],  // BLS12-381 aggregate sig over oracle committee
}
```

### Relay flow

```
ZBX oracle finalises round
  ↓ 7 new modules compute price (TWAP, circuit breaker, heartbeat, Merkle proof)
ZBX-XCM dispatcher (multi_chain.rs)
  ↓ constructs RelayMessage with BLS aggregate sig
  ↓ dispatches to each target chain in parallel
ZbxAggregator.sol (on target chain)
  ↓ verifies BLS sig against oracle committee public key
  ↓ updates latestRoundData()
DeFi protocols on target chain
  ↓ call latestRoundData() — same interface as Chainlink
```

### Chainlink compatibility

Any protocol currently using Chainlink on Ethereum, BSC, Polygon, Arbitrum,
Optimism, or Avalanche can switch to ZBX oracle data with **zero code changes**:

```solidity
// Before (Chainlink):
AggregatorV3Interface feed = AggregatorV3Interface(0xChainlinkAddr);

// After (ZBX oracle relay — same interface):
AggregatorV3Interface feed = AggregatorV3Interface(0xZbxAggregatorAddr);

(, int256 price,,,) = feed.latestRoundData(); // identical call
```

### Stale relay detection

`MultiChainRegistry` tracks `last_relay_timestamp` per network.
A relay is marked stale if `now − last_relay > 2 × heartbeat` for the slowest feed.
Stale feeds trip the circuit breaker on the target chain.

---

## Roadmap

| Milestone | Target | Notes |
|-----------|--------|-------|
| Bridge mainnet | Launch | ETH/BSC/Polygon connectivity |
| XCL FT-1 mainnet | Launch | Trustless token transfer |
| XCL MSG-1 mainnet | Launch | Arbitrary cross-chain messaging ← **Session 20** |
| ZBX-XCM oracle relay | v0.3 | 8-network price relay ← **Session 40** |
| XCL replaces bridge | v0.4 | Bridge sunset as XCL matures |
| XCM (Polkadot) | v0.5 | SCALE-encoded XCM via zbx-codec |
