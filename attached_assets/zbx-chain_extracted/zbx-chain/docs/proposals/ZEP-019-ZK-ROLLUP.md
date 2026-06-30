# ZEP-019: ZK Rollup Native Support + STARK Verifier

| Field       | Value                                        |
|-------------|----------------------------------------------|
| ZEP         | 019                                          |
| Title       | ZK Rollup Native Support + STARK Verifier    |
| Author      | Zebvix Core Team                             |
| Status      | ACCEPTED                                     |
| Category    | Core                                         |
| Created     | 2026-05-05                                   |
| Activation  | Block 300,000                                |

---

## Abstract

ZBX Chain adds native ZK proof verification infrastructure: (1) EVM precompiles
for Groth16, PLONK, and STARK proof verification, (2) native blob transaction
support (EIP-4844 style) for cheap L2 data availability, and (3) a ZK rollup
settlement interface allowing L2 chains to settle on ZBX Chain with trustless
ZK proofs. This makes ZBX Chain an L2-friendly settlement layer.

---

## Motivation

ZK rollups are the dominant L2 scaling solution. To attract rollup deployments
and developers to the ZBX ecosystem, the chain must provide:
- Cheap proof verification (precompiles instead of expensive EVM opcodes)
- Cheap data availability (blob transactions)
- Standard settlement interface (ZK rollup bridge)

Ethereum EIP-4844 reduced rollup costs by 10-100x. ZBX Chain implements an
equivalent system from the start.

---

## Specification

### 1. ZK Precompiles

New EVM precompiles for ZK proof verification:

| Address | Name              | Input                          | Gas Cost  |
|---------|-------------------|--------------------------------|-----------|
| `0x0C`  | BLS_AGG_VERIFY    | (see ZEP-016)                  | 46,000    |
| `0x0D`  | GROTH16_VERIFY    | (vk, proof, public_inputs)     | 150,000   |
| `0x0E`  | PLONK_VERIFY      | (vk, proof, public_inputs)     | 200,000   |
| `0x0F`  | STARK_VERIFY      | (config, proof, public_inputs) | 300,000   |
| `0x10`  | POSEIDON_HASH     | (inputs: Vec<Fp>)              | 200/input |
| `0x11`  | PEDERSEN_COMMIT   | (value, blinding_factor)       | 3,000     |

### 2. Blob Transactions (Data Availability)

Inspired by EIP-4844, ZBX adds blob-carrying transactions:

```rust
pub struct BlobTransaction {
    // All EIP-1559 fields +
    pub blob_versioned_hashes: Vec<H256>,   // commitments to blobs
    pub max_fee_per_blob_gas: u128,
    // Blobs carried in sidecar (not in block body)
}

pub struct BlobSidecar {
    pub blobs: Vec<Blob>,               // each blob = 128 KB
    pub commitments: Vec<KzgCommitment>,// KZG commitment per blob
    pub proofs: Vec<KzgProof>,          // KZG proof per blob
}
```

- Blob data available for 30 days (pruned from full nodes after)
- L2s post their state diff as blobs → 100x cheaper than calldata
- KZG commitments in block header → light clients can verify availability

**Blob gas market**: separate fee market from execution gas:
```
blob_base_fee = prev_blob_base_fee × e^((blob_used - target_blob_per_block) / 64)
target = 3 blobs/block, max = 6 blobs/block
```

### 3. STARK Verifier

STARKs (Scalable Transparent ARgument of Knowledge) require no trusted setup:

```rust
pub struct StarkConfig {
    pub field_modulus: U256,     // stark-friendly prime (2^64 - 2^32 + 1)
    pub blowup_factor: u32,      // FRI security parameter (typically 4-8)
    pub num_queries:   u32,      // FRI query count (security bits / log2(blowup))
    pub proof_of_work_bits: u32, // grinding protection
}

pub struct StarkProof {
    pub trace_commitment:  Vec<u8>,  // Merkle root of execution trace
    pub constraint_polys:  Vec<u8>,  // constraint polynomial evaluations
    pub fri_layers:        Vec<FriLayer>,
    pub fri_remainder:     Vec<u8>,
    pub proof_of_work:     u64,
}

pub fn stark_verify(
    config: &StarkConfig,
    proof: &StarkProof,
    public_inputs: &[U256],
) -> Result<(), StarkError>;
```

### 4. ZK Rollup Settlement Interface

L2 operators post ZK proofs to settle on ZBX Chain:

```solidity
interface IZkRollupSettlement {
    struct RollupState {
        bytes32 l2StateRoot;      // L2 state Merkle root
        uint256 batchIndex;       // sequential batch number
        uint256 l2BlockNumber;    // L2 block height
        bytes32 prevStateRoot;    // previous batch root
    }

    function submitBatch(
        RollupState calldata state,
        bytes calldata zkProof,        // Groth16/PLONK/STARK proof
        bytes calldata publicInputs    // state transition witnesses
    ) external;

    function withdrawFromL2(
        address recipient,
        uint256 amount,
        bytes calldata inclusionProof  // Merkle proof of withdrawal on L2
    ) external;
}
```

### 5. Poseidon Hash Precompile

Poseidon is a ZK-friendly hash (much cheaper to prove than Keccak):

```
Poseidon(x₁, x₂, ..., xₙ) → y ∈ Fp
```

- Used in ZK circuits instead of Keccak (10x cheaper constraints)
- Precompile enables on-chain Poseidon verification
- Compatible with StarkNet, Circom, and ZKSync circuits

---

## Implementation

**Crate**: `zbx-zk` — add `stark.rs` module
**Crate**: `zbx-da` — upgrade for blob transaction support
**Crate**: `zbx-evm` — add new precompiles 0x0D-0x11

```
zbx-zk/src/
├── stark.rs           # NEW: STARK verifier (FRI-based)
├── poseidon.rs        # NEW: Poseidon hash for ZK circuits
├── verifier.rs        # UPGRADED: Groth16 (existing) + PLONK (existing)
├── blob.rs            # NEW: KZG commitment operations
```

---

## Rollup Ecosystem Support

ZBX Chain as settlement layer supports:
- **Type 1 ZK-EVM**: full EVM-equivalent (e.g. Polygon zkEVM)
- **Type 2 ZK-EVM**: EVM-equivalent with different state representation
- **StarkEx/StarkNet**: STARK-based rollups
- **Custom app rollups**: ZK proofs for specific applications (DEX, game)

---

## References

- EIP-4844: Shard Blob Transactions
- EIP-1108: Reduce alt_bn128 precompile gas costs
- StarkWare STARK paper: https://eprint.iacr.org/2018/046
- KZG commitments: https://dankradfeist.de/ethereum/2020/06/16/kate-polynomial-commitments.html
