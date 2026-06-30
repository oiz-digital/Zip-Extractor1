# ZEP-018: MEV Protection

| Field       | Value                                        |
|-------------|----------------------------------------------|
| ZEP         | 018                                          |
| Title       | MEV Protection — Encrypted Mempool + PBS     |
| Author      | Zebvix Core Team                             |
| Status      | ACCEPTED                                     |
| Category    | Core                                         |
| Created     | 2026-05-05                                   |
| Activation  | Block 150,000                                |

---

## Abstract

ZBX Chain implements a four-layer MEV protection strategy: (1) encrypted private
mempool, (2) commit-reveal ordering, (3) Proposer-Builder Separation (PBS),
and (4) MEV redistribution to stakers and the community fund. Together these
layers eliminate sandwich attacks, frontrunning, and validator-level MEV
extraction while capturing and redistributing any remaining MEV fairly.

---

## Motivation

MEV (Maximal Extractable Value) in 2023-2025 exceeded $1B on Ethereum alone.
Common attacks:
- **Sandwich attacks**: bot wraps a user swap with buy/sell → user gets worse price
- **Frontrunning**: bot copies a profitable tx and pays higher gas to go first
- **Backrunning**: bot trades immediately after a large price-moving tx
- **Time-bandit attacks**: validators reorg history to extract MEV

ZBX Chain's early action prevents MEV from becoming entrenched.

---

## Specification

### Layer 1: Private Mempool (Encrypted Tx Submission)

Users submit transactions via `zbx_sendPrivateTransaction` RPC:

```
Client → Encrypt(tx, proposer_pubkey) → ZBX Node
```

- Transaction content encrypted with proposer's ephemeral pubkey
- Only the proposer for the current slot can decrypt
- Proposer decrypts at block sealing time — content invisible to other validators
- After block is finalized, tx revealed publicly (on-chain data availability)

**Encryption**: X25519-ChaCha20-Poly1305 (authenticated encryption)

```rust
pub struct PrivateTx {
    pub encrypted_tx: Vec<u8>,          // ChaCha20-Poly1305 ciphertext
    pub ephemeral_pubkey: [u8; 32],     // X25519 ephemeral key
    pub nonce: [u8; 12],                // ChaCha20 nonce
    pub target_slot: u64,               // which block slot this is for
    pub tip: u128,                      // visible gas tip (for ordering)
}
```

### Layer 2: Commit-Reveal Ordering

For transactions requiring ordering protection (e.g. DEX swaps):

```
Block N:   User submits tx_hash (commitment) — content hidden
Block N+1: User reveals tx content — included in this block

Rule: reveals processed BEFORE new commitments in each block
```

```rust
pub struct CommitReveal {
    pub commit_block: u64,
    pub tx_hash: H256,         // commitment in block N
    pub tx_data: Option<Vec<u8>>, // revealed in block N+1
    pub author: Address,
}
```

Prevents last-second frontrunning: by the time a frontrunner sees tx content,
the commit is already finalized and they cannot insert before it.

### Layer 3: Proposer-Builder Separation (PBS)

Separates block proposal from block building:

```
┌─────────────────────────────────────────────────────────┐
│                      Validators                          │
│   Propose slot auctions → receive highest bid → sign    │
│   (Validators NEVER see tx content before signing)      │
└─────────────────────────────────────────────────────────┘
              ↑ sealed block header
┌─────────────────────────────────────────────────────────┐
│                   Block Builders                         │
│   Compete to fill blocks → submit sealed bids           │
│   Builder with highest bid wins the slot                 │
│   Builder sees tx content, but cannot steal MEV         │
│   (their block is committed before revealing)           │
└─────────────────────────────────────────────────────────┘
```

```rust
pub struct BuilderBid {
    pub slot:        u64,
    pub block_root:  H256,    // Merkle root of proposed block
    pub value:       u128,    // ZBX payment to validator
    pub builder:     Address,
    pub signature:   Signature,
}
```

Builders compete on value — MEV captured by builder is partially redistributed
(see Layer 4). Validators get the bid value without needing to extract MEV.

### Layer 4: MEV Redistribution

MEV captured via PBS bids is redistributed:

```
MEV Revenue
    ├── 30% → Stakers (proportional to stake)
    ├── 50% → Community Fund (governance controlled)
    └── 20% → Builder reward (after paying validator bid)
```

```rust
pub struct MevRedistribution {
    pub total_mev_wei:   u128,
    pub staker_share:    u128,   // 30%
    pub community_share: u128,   // 50%
    pub builder_share:   u128,   // 20%
    pub epoch:           u64,
}
```

Distribution happens at epoch boundary (every 172,800 blocks).

### Layer 5: FIFO Fair Ordering (Optional Per-Block)

Block proposers may optionally enable FIFO ordering within a block:
- Transactions ordered by arrival timestamp at proposer node
- Gas price tie-breaking only within same timestamp window (1s)
- Prevents pure gas-auction frontrunning

---

## Implementation

**Crate**: `zbx-mev` (already exists — upgrading all modules)

```
zbx-mev/src/
├── private_pool.rs    # UPGRADED: X25519 encryption
├── commit_reveal.rs   # UPGRADED: full state machine
├── pbs.rs             # UPGRADED: full PBS relay
├── redistribution.rs  # UPGRADED: epoch distribution
├── builder.rs         # UPGRADED: builder registry
├── bundle.rs          # UPGRADED: bundle simulation
└── error.rs
```

---

## RPC Methods

| Method                        | Description                           |
|-------------------------------|---------------------------------------|
| `zbx_sendPrivateTransaction`  | Submit encrypted tx                   |
| `zbx_getPrivateTxStatus`      | Check if private tx was included      |
| `zbx_getBuildersForSlot`      | List registered builders for slot     |
| `zbx_getMevStats`             | Epoch MEV statistics                  |

---

## Security

- **Proposer collusion**: mitigated by PBS — proposer never sees tx content
- **Builder frontrunning**: commit-reveal prevents even builders from frontrunning
- **Late decryption**: if proposer fails to decrypt, tx re-enters next slot's pool
- **Builder bid manipulation**: bids are signed and verified on-chain

---

## References

- Flashbots MEV-Boost: https://boost.flashbots.net/
- EIP-4337 bundler economics
- SUAVE (Single Unified Auction for Value Expression): https://suave.flashbots.net/
