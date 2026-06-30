# ZEP-021: State Expiry + Verkle Trees

| Field       | Value                                        |
|-------------|----------------------------------------------|
| ZEP         | 021                                          |
| Title       | State Expiry + Verkle Tree Migration         |
| Author      | Zebvix Core Team                             |
| Status      | ACCEPTED                                     |
| Category    | Core                                         |
| Created     | 2026-05-05                                   |
| Activation  | Block 150,000 (Verkle dual-mode), 200,000 (Verkle-only), 300,000 (expiry) |

---

## Abstract

ZBX Chain addresses unbounded state growth via two mechanisms: (1) **Verkle
Trees** replace Merkle-Patricia Tries for 10-100x smaller state proofs enabling
stateless clients, and (2) **State Expiry** automatically removes dormant
storage slots after 2 years unless rent is paid, keeping state size bounded.
Together these make ZBX Chain sustainable for decades without pruning.

---

## Motivation

Ethereum's state has grown to >160 GB (2026). A new full node must download
and verify all historical state. Without intervention, ZBX Chain state would
grow similarly. ZBX Chain addresses this proactively from the start.

**Problems solved**:
- Full node sync time: weeks → hours (smaller state + Verkle witnesses)
- State proof size: 3KB → 100-200 bytes (Verkle vs Merkle)
- Stateless clients: possible with Verkle (impossible with MPT)
- State bloat: bounded with expiry + rent

---

## Specification

### Part 1: Verkle Trees

#### 1.1 Tree Structure

```
Verkle tree: 256-ary (each node has up to 256 children, byte-indexed)
Key: 32 bytes → path from root to leaf
Value: 32 bytes

Internal node: Pedersen vector commitment over 256 children
  C = com(C₀, C₁, ..., C₂₅₅) using IPA (Inner Product Argument)
Leaf node: (stem[31], suffix_tree_commitment)
```

#### 1.2 Proof Format

```rust
pub struct VerkleProof {
    /// IPA proof for all queried keys (aggregated)
    pub ipa_proof: IpaProof,
    /// Pre-state values for all keys queried
    pub pre_values: Vec<Option<[u8; 32]>>,
    /// Keys queried in this proof
    pub keys: Vec<[u8; 32]>,
    /// Commitments along each proof path
    pub commitments: Vec<CompressedG1>,
}
```

Single VerkleProof covers ALL state reads in a block (~200 bytes vs ~3KB per
key in MPT). This is the key enabler for stateless clients.

#### 1.3 State Layout in Verkle Tree

```
Account stem = keccak256(address)[0..31]  (31 bytes)

Suffixes:
  stem ++ 0 → account nonce
  stem ++ 1 → account balance
  stem ++ 2 → account storage version
  stem ++ 3 → account code hash
  stem ++ 4 → account code size
  stem ++ 64..255 → first 192 storage slots (inline)
  keccak256(stem ++ storage_key)[0..31] → rest of storage
```

#### 1.4 Migration Timeline

| Block    | Action                                                    |
|----------|-----------------------------------------------------------|
| 150,000  | Dual-mode: reads from MPT, writes to BOTH MPT + Verkle   |
| 200,000  | Verkle-only: MPT retired, state root = Verkle root        |
| 200,001+ | Stateless clients can sync using only Verkle witnesses    |

### Part 2: State Expiry

#### 2.1 Rent Model

Each 32-byte storage slot incurs annual rent:

```
slot_rent = 0.0001 ZBX / year = 100,000,000,000,000 wei / year
           = ~274 micro-ZBX / day
           ≈ $0.000274/day at ZBX=$1
```

Rent collected at first access after dormancy period ends.

#### 2.2 Expiry Rules

```rust
pub enum SlotState {
    Active,         // accessed within last ACTIVE_PERIOD blocks
    Dormant,        // not accessed, but rent is paid
    Hibernated,     // balance < min_balance; slot removed from active trie
    Expired,        // dormant for > EXPIRY_BLOCKS; permanently pruned
}

pub const ACTIVE_PERIOD:   u64 = 6_307_200;  // ~1 year at 5s/block
pub const EXPIRY_BLOCKS:   u64 = 12_614_400; // ~2 years at 5s/block
pub const FREE_SLOTS:      u64 = 5;          // first 5 slots free (small accounts)
pub const MIN_BALANCE_WEI: u128 = 10_000_000_000_000_000; // 0.01 ZBX
```

#### 2.3 Revival Mechanism

Expired slots can be revived by providing:
1. The expired value (from archive node or user's own records)
2. A Merkle proof that the value was in state before expiry
3. Payment of revival fee + back-rent

```rust
pub struct RevivalProof {
    pub address: Address,
    pub slot: H256,
    pub value: H256,                  // original value
    pub expiry_proof: HistoricalProof, // proof from archive
    pub back_rent: u128,              // rent owed since last access
}
```

#### 2.4 Exemptions

- System contracts (genesis addresses 0x0000...0001 through 0x0000...00FF)
- Staking contracts (as long as validator is active)
- ZUSD contract state
- First 5 storage slots of any account (`FREE_SLOTS`)

---

## Implementation

**Crates**: `zbx-verkle` and `zbx-state-rent` (both already exist — upgrading)

```
zbx-verkle/src/
├── tree.rs         # UPGRADED: full IPA commitment
├── proof.rs        # UPGRADED: aggregated multiproof
├── migration.rs    # NEW: dual-mode MPT+Verkle transition

zbx-state-rent/src/
├── rent.rs         # UPGRADED: full rent accounting
├── scheduler.rs    # UPGRADED: expiry checker per block
├── revival.rs      # UPGRADED: revival proof verification
```

---

## Storage Savings (Estimate)

| Metric                    | MPT (no expiry) | Verkle + Expiry |
|---------------------------|-----------------|-----------------|
| State size at 1M blocks   | ~80 GB          | ~15 GB          |
| State proof per tx        | ~3 KB           | ~150 bytes      |
| Full node sync time       | ~72 hours       | ~6 hours        |
| Stateless client support  | No              | Yes             |

---

## References

- Vitalik on state expiry: https://notes.ethereum.org/@vbuterin/state_expiry_eip
- Verkle trees: https://vitalik.eth.limo/general/2021/06/18/verkle.html
- EIP-6800: Ethereum state using Verkle trees
