# ZEP-026: Native Cross-Chain Messaging

| Field       | Value                                          |
|-------------|------------------------------------------------|
| ZEP         | 026                                            |
| Title       | Native Cross-Chain Messaging (XCL v2)          |
| Author      | Zebvix Core Team                               |
| Status      | ACCEPTED                                       |
| Category    | Core                                           |
| Created     | 2026-05-05                                     |
| Activation  | Block 300,000                                  |

---

## Abstract

ZBX Chain's native Cross-Chain Layer (XCL) — already implemented in
`zbx-xcl` — is formalized and upgraded to XCL v2 with: (1) ZK-verified
packet proofs replacing BLS light client verification for L2 rollups,
(2) arbitrary message passing (not just token transfers), (3) native
IBC compatibility (from ZEP-024), and (4) a ZBX gas abstraction layer
allowing cross-chain txs to be paid in the source chain's native token.

---

## Motivation

Cross-chain interoperability is essential for a multi-chain ecosystem.
The existing XCL (`zbx-xcl`) implements trustless packet relay using
BLS light client verification. ZEP-026 extends this with:
- **ZK verification**: more efficient for ZK rollup settlements
- **Arbitrary messaging**: not just token transfers
- **IBC standard**: interop with 50+ Cosmos chains
- **Gas abstraction**: pay in any token, convert to ZBX automatically

---

## Specification

### 1. XCL Architecture (Current → XCL v2)

```
XCL v1 (existing):
  Packet → BLS light client verify → MPT proof verify → execute

XCL v2 additions:
  Packet → [BLS light client verify | ZK proof verify] → execute
         → Arbitrary message dispatch (not just token transfers)
         → Gas abstraction (pay in source token)
         → IBC channel compatibility
```

### 2. Packet Format (v2)

```rust
pub struct XclPacketV2 {
    /// Protocol version (2 for XCL v2)
    pub version: u8,
    /// Unique sequence number per channel
    pub sequence: u64,
    /// Source chain identifier
    pub source_chain: ChainId,
    pub source_channel: ChannelId,
    pub source_port: PortId,
    /// Destination chain identifier  
    pub dest_chain: ChainId,
    pub dest_channel: ChannelId,
    pub dest_port: PortId,
    /// Application-specific payload
    pub app: XclApp,
    /// Timeout (either block height or timestamp)
    pub timeout: XclTimeout,
    /// Gas budget for execution on destination
    pub dest_gas: u64,
    /// Proof verification method preferred by source chain
    pub proof_type: ProofType,
}

pub enum XclApp {
    /// Fungible token transfer (FT-1)
    FungibleTransfer(FtPacketData),
    /// NFT transfer (NFT-1)
    NftTransfer(NftPacketData),
    /// Arbitrary message (MSG-1)
    ArbitraryMessage(ArbitraryMsgData),
    /// Contract call (CALL-1)
    ContractCall(ContractCallData),
}

pub enum ProofType {
    BlsLightClient,   // verify via BLS aggregate QC
    ZkProof,          // verify via ZK validity proof (for ZK rollups)
    IbcTendermint,    // verify via IBC Tendermint light client
}
```

### 3. Arbitrary Message Passing

Any smart contract can send/receive cross-chain messages:

```solidity
interface IXclReceiver {
    function onXclReceive(
        uint8   version,
        string  memory sourceChain,
        string  memory sourceAddress,
        bytes   memory payload
    ) external returns (bytes4);
}

// Return value must be:
bytes4 constant SUCCESS = 0x12345678;
```

Sender:
```solidity
// Any contract can call via XCL precompile (0x0b)
IXclSender(XCL_PRECOMPILE).send{value: fee}(
    "cosmos-hub-1",           // destination chain ID
    "cosmos1abc...xyz",       // destination address
    abi.encode(myPayload),    // arbitrary bytes payload
    1_000_000                 // destination gas limit
);
```

### 4. ZK Proof Verification for Rollup Packets

For ZK rollup chains settling on ZBX, packets are verified via ZK proof
instead of BLS light client (more efficient for chains without validators):

```rust
pub enum PacketVerification {
    /// Standard: verify BLS QC from source chain validators
    BLSLightClient {
        header: LightHeader,
        proof: MerkleProof,
    },
    /// ZK Rollup: verify ZK proof of source chain state transition
    ZKProof {
        state_root: H256,
        zk_proof: GrothOrStark,
        public_inputs: Vec<U256>,
    },
}
```

### 5. Gas Abstraction

Cross-chain transactions can pay fees in source chain's token:

```
Source chain: User sends ATOM for a cross-chain tx to ZBX Chain
ZBX Chain:
  1. Relayer fronts ZBX gas cost
  2. Packet contains { gas_token: "ibc/ATOM", gas_amount: X }
  3. On execution: ATOM converted to ZBX via oracle price
  4. Relayer receives ATOM equivalent + small spread (0.1%)
```

```rust
pub struct GasAbstraction {
    pub source_token: Denom,    // token user pays in (e.g. "uatom")
    pub source_amount: u128,    // amount in source token
    pub oracle_price: Price,    // ZBX/source_token rate at time of relay
    pub relayer_fee: u128,      // 0.1% of conversion
}
```

### 6. Channel Types

```rust
pub enum ChannelOrdering {
    /// Ordered: packets delivered in sequence (no gaps)
    Ordered,
    /// Unordered: packets can arrive out of sequence
    Unordered,
}

pub enum ChannelSecurity {
    /// Require proof for each packet (standard)
    PerPacket,
    /// Batch proofs for efficiency (for high-throughput channels)
    BatchProof { batch_size: u32 },
    /// ZK proof covers entire batch
    ZKBatch,
}
```

### 7. Timeout Handling

```rust
pub enum XclTimeout {
    /// Timeout if packet not delivered by block height
    Height(IbcHeight),
    /// Timeout if packet not delivered by timestamp
    Timestamp(u64),
    /// Both (whichever comes first)
    Both { height: IbcHeight, timestamp: u64 },
}

/// If timeout reached without delivery:
/// - Source chain: refund sender (tokens released from escrow)
/// - Relayer: no reward (incentive to relay promptly)
```

### 8. Relayer Economics

```
Relayer submits packet proof → receives:
  base_reward   = gas_used × gas_price           (covered by user)
  relay_bonus   = 0.01% of packet value          (incentive)
  priority_fee  = optional tip from sender        (for fast relay)
```

Relayers are permissionless — anyone can relay.
Multiple relayers compete → fastest relay gets the reward.

---

## Implementation

**Crate**: `zbx-xcl` (already exists — adding v2 features)

```
zbx-xcl/src/
├── packet.rs        # UPGRADED: XclPacketV2 type
├── handler.rs       # UPGRADED: arbitrary message dispatch
├── client.rs        # UPGRADED: ZK proof verification path
├── transfer.rs      # UPGRADED: gas abstraction
├── relay.rs         # UPGRADED: batch relay support
├── ibc_compat.rs   # NEW: IBC channel/connection compatibility shim
```

---

## Supported Chains (Launch)

| Chain           | Protocol | Proof Type      | Status    |
|-----------------|----------|-----------------|-----------|
| Cosmos Hub      | IBC      | Tendermint LC   | Launch    |
| Osmosis         | IBC      | Tendermint LC   | Launch    |
| Ethereum L1     | XCL      | ZK (SP1/Groth16)| Q2 2026   |
| Polygon zkEVM   | XCL      | ZK (PLONK)      | Q3 2026   |
| StarkNet        | XCL      | STARK           | Q3 2026   |
| Solana          | XCL      | BLS LC          | Q4 2026   |

---

## References

- IBC Protocol: https://ibcprotocol.dev/
- ZBX XCL implementation: `crates/zbx-xcl/src/`
- LayerZero arbitrary messaging: https://layerzero.network/
- CCIP (Chainlink): https://chain.link/cross-chain
