# ZEP-016: BLS Signature Aggregation

| Field       | Value                                  |
|-------------|----------------------------------------|
| ZEP         | 016                                    |
| Title       | BLS12-381 Signature Aggregation        |
| Author      | Zebvix Core Team                       |
| Status      | ACCEPTED                               |
| Category    | Core                                   |
| Created     | 2026-05-05                             |
| Activation  | Block 200,000                          |

---

## Abstract

Replace per-validator individual signatures in block headers with a single
**BLS12-381 aggregate signature** covering all voting validators. This reduces
block header size by ~95% for a 100-validator set, cuts verification time from
O(n) ECDSA verifications to O(1) pairing check, and enables efficient
**validator set attestations** for light clients.

---

## Motivation

Current state:
- Each validator signs a vote with ECDSA secp256k1 (64 bytes each)
- 100 validators → 6,400 bytes of signatures per block header
- Verification: 100 individual ECDSA recoveries

With BLS aggregation:
- All 100 signatures combine into 1 BLS signature (96 bytes)
- Verification: 2 pairing operations (constant time regardless of n)
- 98.5% reduction in signature data
- Light clients can verify finality with a single pairing check

---

## Specification

### 1. BLS12-381 Curve Parameters

```
Curve:       BLS12-381
Field:       Fp (381-bit prime)
G1 points:   48 bytes compressed (public keys)
G2 points:   96 bytes compressed (signatures)
Pairing:     e: G1 × G2 → GT
Security:    ~128-bit classical, ~64-bit quantum
```

### 2. Signature Scheme

```
KeyGen():
  sk ∈ Fp (random scalar)
  pk = sk · G1  (48 bytes compressed)

Sign(sk, msg):
  H = hash_to_G2(msg)       # RFC 9380 hash-to-curve
  σ = sk · H                 # 96 bytes compressed

Verify(pk, msg, σ):
  H = hash_to_G2(msg)
  return e(pk, H) == e(G1, σ)

Aggregate(σ₁, σ₂, ..., σₙ):
  σ_agg = σ₁ + σ₂ + ... + σₙ  # G2 point addition (96 bytes)

AggVerify([pk₁..pkₙ], [msg₁..msgₙ], σ_agg):
  # n+1 pairings — used when all messages differ
  ∏ e(pkᵢ, H(msgᵢ)) == e(G1, σ_agg)

FastAggVerify([pk₁..pkₙ], msg, σ_agg):
  # 2 pairings — used when all validators sign same block hash
  pk_agg = pk₁ + pk₂ + ... + pkₙ
  return e(pk_agg, H(msg)) == e(G1, σ_agg)
```

### 3. Block Header Changes

```rust
pub struct QuorumCertificate {
    pub block_hash: H256,
    pub round:      u64,
    pub phase:      Phase,

    // OLD: Vec<(Address, Signature)> — grows with validator count
    // NEW: BLS aggregate
    pub agg_signature: BlsAggSig,      // 96 bytes
    pub signer_bitmap: ValidatorBitmap, // n/8 bytes (bitfield of who signed)
}

pub struct ValidatorBitmap(pub Vec<u8>); // 1 bit per validator slot
```

### 4. Validator Registration

Each validator registers a BLS public key alongside their ECDSA key:

```rust
pub struct ValidatorInfo {
    pub address:    Address,    // ECDSA address (existing)
    pub bls_pubkey: BlsPubKey,  // NEW: 48-byte G1 point
    pub stake:      u128,
}
```

Registration tx includes a **proof of possession** (PoP) — a BLS signature of
the validator's ECDSA address using the BLS private key — to prevent rogue key
attacks:

```
pop = BLS_Sign(bls_sk, keccak256(ecdsa_address || "zbx-bls-pop-v1"))
```

### 5. Aggregation Process (Block Production)

```
1. Proposer broadcasts block to all validators
2. Each validator:
   a. Verifies block
   b. Signs block_hash with BLS private key
   c. Broadcasts BLS vote
3. Proposer collects 2f+1 BLS votes:
   a. σ_agg = σ₁ + σ₂ + ... + σ_(2f+1)
   b. bitmap = set bits for each signer
4. QC = { block_hash, round, phase, σ_agg, bitmap }
5. QC embedded in next block header
```

### 6. Verification

```rust
fn verify_qc(qc: &QuorumCertificate, validator_set: &ValidatorSet) -> bool {
    let signers = validator_set.from_bitmap(&qc.signer_bitmap);
    if signers.len() < validator_set.quorum() { return false; }
    let pk_agg: BlsPubKey = signers.iter().map(|v| v.bls_pubkey).aggregate();
    bls_fast_agg_verify(&pk_agg, &qc.block_hash.0, &qc.agg_signature)
}
```

### 7. Batch Verification

For sync nodes processing many blocks:

```rust
fn batch_verify_qcs(qcs: &[QuorumCertificate], vs: &ValidatorSet) -> bool {
    // Miller loop batching: verify N QCs in ~2 pairings instead of 2N
    bls_batch_verify(qcs.iter().map(|qc| (&qc.agg_pk, &qc.block_hash, &qc.agg_signature)))
}
```

---

## Implementation

**Crate**: `zbx-threshold` — new module `bls_aggregate`
**Also**: `zbx-crypto/src/bls.rs` upgrade

```
zbx-threshold/src/
├── bls_aggregate.rs   # NEW: aggregate/verify APIs
├── aggregate.rs       # UPGRADED: use BLS aggregate
```

---

## Gas Cost (EVM Precompile)

New precompile `0x0C: BLS_AGG_VERIFY`:

| Operation         | Gas Cost |
|-------------------|----------|
| Point add G1      | 500      |
| Point add G2      | 800      |
| Pairing           | 45,000   |
| Hash to G2        | 110,000  |
| Fast agg verify   | 46,000   |

---

## Security

- **Rogue key attack**: mitigated by proof-of-possession requirement
- **Subgroup check**: all points checked to be in correct subgroup
- **Constant time**: pairing verification is constant-time

---

## References

- IETF BLS Signatures: https://datatracker.ietf.org/doc/draft-irtf-cfrg-bls-signature/
- EIP-2537 BLS12-381 precompiles
- Ethereum beacon chain BLS spec
