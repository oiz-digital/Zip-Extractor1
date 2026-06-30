# ZEP-024: Light Client Protocol + IBC Compatibility

| Field       | Value                                        |
|-------------|----------------------------------------------|
| ZEP         | 024                                          |
| Title       | Light Client Protocol + IBC Compatibility    |
| Author      | Zebvix Core Team                             |
| Status      | ACCEPTED                                     |
| Category    | Core                                         |
| Created     | 2026-05-05                                   |
| Activation  | Block 200,000                                |

---

## Abstract

ZBX Chain ships a production-ready light client protocol enabling mobile
wallets and browsers to verify block finality and transaction inclusion
without running a full node. The protocol uses BLS aggregate signatures
(ZEP-016) for O(1) finality verification and is compatible with the
**IBC (Inter-Blockchain Communication)** standard, enabling ZBX Chain
to communicate with any IBC-enabled chain (Cosmos ecosystem, etc.).

---

## Motivation

Full nodes require 10-50 GB storage and hours of sync time. Light clients
enable:
- Mobile wallet users to verify their own transactions
- Browser-based dapps to trust chain state without a centralized RPC
- IBC cross-chain communication with Cosmos ecosystem
- Trustless bridges that verify ZBX state on other chains

---

## Specification

### 1. Light Client Data Model

```rust
pub struct LightHeader {
    pub height:        u64,
    pub hash:          H256,
    pub parent_hash:   H256,
    pub state_root:    H256,        // Verkle root (from ZEP-021)
    pub tx_root:       H256,
    pub receipts_root: H256,
    pub timestamp:     u64,
    pub proposer:      Address,
    /// BLS aggregate QC from 2f+1 validators (from ZEP-016)
    pub quorum_cert:   QuorumCertificate,
    /// Verkle state witness for this block's reads (from ZEP-021)
    pub state_witness: Option<VerkleWitness>,
}

pub struct Checkpoint {
    pub height:      u64,
    pub hash:        H256,
    pub validator_set_hash: H256,  // commitment to current validator set
    pub timestamp:   u64,
}
```

### 2. Finality Verification

A light client verifies block finality in O(1):

```rust
pub fn verify_finality(
    header: &LightHeader,
    validator_set: &ValidatorSet,
) -> Result<(), LightClientError> {
    // 1. Verify QC aggregate signature
    let signers = validator_set.from_bitmap(&header.quorum_cert.signer_bitmap);
    if signers.len() < validator_set.quorum() {
        return Err(LightClientError::InsufficientSigners);
    }
    let pk_agg = bls_aggregate_pubkeys(signers.iter().map(|v| &v.bls_pubkey));
    bls_fast_agg_verify(&pk_agg, &header.hash.0, &header.quorum_cert.agg_signature)?;

    // 2. Verify parent chain (check parent_hash links)
    // 3. Verify timestamp is monotonically increasing
    Ok(())
}
```

Single pairing check — takes ~1ms on mobile hardware.

### 3. Sync Protocol

```rust
pub enum SyncRequest {
    /// Get the latest finalized header
    LatestHeader,
    /// Get a range of headers
    Headers { from: u64, to: u64 },
    /// Get headers since a checkpoint
    HeadersSince { checkpoint: Checkpoint },
    /// Request state proof for an account
    AccountProof { address: Address, height: u64 },
    /// Request tx inclusion proof
    TxProof { tx_hash: H256, height: u64 },
}

pub struct LightSync {
    pub trusted_checkpoint: Checkpoint,  // hardcoded genesis or last verified
    pub headers: HeaderChain,
    pub peers: Vec<PeerAddr>,
}

impl LightSync {
    pub async fn sync_to_latest(&mut self) -> Result<LightHeader, LightClientError>;
    pub async fn verify_tx(&self, tx_hash: H256) -> Result<TxProof, LightClientError>;
    pub async fn verify_account(&self, addr: Address) -> Result<AccountProof, LightClientError>;
}
```

### 4. IBC Client State (Cosmos Compatibility)

ZBX Chain implements the IBC `ClientState` and `ConsensusState` interfaces:

```rust
/// IBC-compatible client state stored on a counterparty chain.
pub struct ZbxClientState {
    pub chain_id:             String,      // "zbx-mainnet-1"
    pub trust_level:          Fraction,    // 1/3 minimum (BFT threshold)
    pub trusting_period:      Duration,    // how long to trust a header (14 days)
    pub unbonding_period:     Duration,    // validator unbonding (from staking)
    pub max_clock_drift:      Duration,    // max timestamp skew allowed (10s)
    pub latest_height:        IbcHeight,
    pub frozen_height:        Option<IbcHeight>, // set if client is frozen
    pub proof_specs:          Vec<ProofSpec>,    // Verkle proof format
}

/// IBC consensus state for a specific height.
pub struct ZbxConsensusState {
    pub timestamp:   Time,
    pub root:        CommitmentRoot,  // Verkle state root
    pub next_validators_hash: H256,   // hash of next validator set
}

pub struct IbcHeight {
    pub revision_number: u64,  // chain epoch (0 for ZBX mainnet)
    pub revision_height: u64,  // block height
}
```

### 5. IBC Connection Handshake

ZBX Chain participates in IBC connections:

```
ConnOpenInit   (ZBX Chain → Counterparty):
  Create connection with proposed counterparty client

ConnOpenTry    (Counterparty → ZBX Chain):
  Verify ZBX client state is valid

ConnOpenAck    (ZBX Chain → Counterparty):
  Acknowledge counterparty's connection

ConnOpenConfirm (Counterparty → ZBX Chain):
  Finalize connection — bidirectional channel established
```

### 6. IBC Packet Relay

Once an IBC connection exists, packets flow via the XCL handler (ZEP-026):

```rust
pub struct IbcPacket {
    pub sequence:           u64,
    pub source_port:        String,     // "transfer"
    pub source_channel:     String,     // "channel-0"
    pub destination_port:   String,
    pub destination_channel: String,
    pub data:               Vec<u8>,    // application-specific payload
    pub timeout_height:     IbcHeight,
    pub timeout_timestamp:  u64,
}
```

### 7. Trust Model

```
Weak subjectivity: light client must have a checkpoint < trusting_period old.
  - Trusting period: 14 days
  - Unbonding period: 14 days (same — standard IBC requirement)
  - If client is offline > 14 days: must re-sync from a trusted checkpoint

Checkpoint sources (in trust order):
  1. Genesis block (hardcoded, always valid)
  2. Sync committee: 1/3 of validators attest to a checkpoint
  3. User-provided checkpoint (manual, requires verification)
```

---

## Implementation

**Crate**: `zbx-light` — new module `ibc.rs`

```
zbx-light/src/
├── header_chain.rs  # UPGRADED: Verkle witness support
├── spv.rs           # UPGRADED: Verkle-based proofs
├── sync.rs          # UPGRADED: optimistic sync
├── ibc.rs           # NEW: IBC client/consensus state + handshake
└── rpc.rs           # UPGRADED: IBC relayer RPC methods
```

---

## IBC Ecosystem Access

With IBC compatibility, ZBX Chain can connect to:
- Cosmos Hub (ATOM)
- Osmosis (DEX)
- Any Cosmos SDK chain
- Ethereum (via IBC-Ethereum bridge)
- Any chain implementing IBC light client standard

---

## References

- IBC Protocol: https://ibcprotocol.dev/
- ICS-002 Client Semantics: https://github.com/cosmos/ibc/blob/main/spec/core/ics-002-client-semantics
- Tendermint Light Client: https://github.com/cosmos/ibc/blob/main/spec/client/ics-007-tendermint-client
