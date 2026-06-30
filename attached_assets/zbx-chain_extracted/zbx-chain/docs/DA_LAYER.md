# Zebvix Chain — Data Availability Layer

## Overview

ZBX Chain implements a native Data Availability (DA) layer that enables:

- **Blob transactions** (Type 0x03, EIP-4844 compatible) for rollup batch posting
- **KZG polynomial commitments** for efficient blob verification
- **Data Availability Sampling (DAS)** for light client DA verification
- **Separate blob fee market** (EIP-4844-style, independent of execution gas)
- **Blob pruning** after 30-day finality window (~518,400 blocks)

---

## Blob Transactions (Type 0x03)

### Format

| Field                     | Description                                        |
|---------------------------|----------------------------------------------------|
| `chain_id`                | 8989 (ZBX mainnet)                                 |
| `nonce`                   | Sender nonce                                       |
| `max_fee_per_gas`         | EIP-1559 execution fee                             |
| `max_fee_per_blob_gas`    | Separate blob fee (wei/byte)                       |
| `to`                      | Rollup inbox contract address                      |
| `blob_versioned_hashes`   | SHA-256(blob) with 0x01 version prefix             |
| `sidecars`                | [blob, KZG commitment, KZG proof] per blob         |

### Limits

| Parameter              | Value                                |
|------------------------|--------------------------------------|
| Max blob size          | 128 KB (4,096 field elements × 32B)  |
| Max blobs per block    | 8 blobs (1 MB total)                 |
| Target blobs per block | 4 blobs (512 KB)                     |
| Blob retention period  | 30 days (~518,400 blocks)            |

---

## KZG Commitments

Blob data is committed using KZG polynomial commitments over BLS12-381:

1. **Trusted Setup**: Uses the Ethereum KZG ceremony (EIP-4844 compatible, 4,096 G1 points)
2. **Commitment**: Each blob is committed as a polynomial evaluated at the G1 trusted points
3. **Proof**: A KZG opening proof at a random point (Fiat-Shamir) proves the commitment
4. **Verification**: Validators verify commitments via a single BLS12-381 pairing check

---

## Data Availability Sampling (DAS)

Light clients can verify DA without downloading full blobs:

1. Light client selects **75 random chunks** from each blob
2. For each chunk, requests the chunk + KZG proof from network peers
3. Verifies each KZG proof against the published commitment
4. 75 samples → **>99.99% detection probability** for any withheld data

### DAS Protocol

```
LightClient  ──► Peer: "Give me chunk [idx] of blob [hash]"
Peer         ──► LightClient: chunk_data + kzg_proof
LightClient: verify kzg_proof(commitment, chunk_data, idx) == true
```

---

## Blob Fee Market

Blob fees follow a separate market independent of execution gas:

```
blob_base_fee(n+1) = blob_base_fee(n) × e^((blobs_used - target) / 3,338,477)
```

- **Target**: 4 blobs/block
- **Max**: 8 blobs/block
- **Minimum blob base fee**: 1 wei/byte

---

## Rollup Integration

To post rollup batches to ZBX DA layer:

```solidity
// Post a rollup batch as blob data
interface IZbxInbox {
    function postBatch(bytes calldata commitment) external;
}
```

```rust
// Off-chain: create blob transaction
let blob = Blob::from_bytes(&rollup_batch_data)?;
let commitment = kzg.blob_to_kzg_commitment(&blob.0[..]);
let tx = BlobTransaction {
    chain_id: 8989,
    to: ROLLUP_INBOX_ADDRESS,
    max_fee_per_blob_gas: current_blob_base_fee,
    blob_versioned_hashes: vec![blob.versioned_hash()],
    sidecars: vec![BlobSidecar { blob, commitment, proof }],
    ..Default::default()
};
```

---

## Configuration

```toml
[da]
# Enable native DA layer
enabled = true
# KZG trusted setup file
kzg_setup = "/etc/zbx/kzg_trusted_setup.json"
# Max blobs per block
max_blobs_per_block = 8
# Blob pruning enabled
pruning = true
# Prune blobs older than N blocks
prune_after_blocks = 518400
# DA sampling for light clients
sampling_enabled = true
samples_per_blob = 75
```