# ZEP-015: Post-Quantum Cryptography

| Field       | Value                                  |
|-------------|----------------------------------------|
| ZEP         | 015                                    |
| Title       | Post-Quantum Cryptography              |
| Author      | Zebvix Core Team                       |
| Status      | ACCEPTED                               |
| Category    | Core                                   |
| Created     | 2026-05-05                             |
| Activation  | Block 500,000 (dual-mode), 750,000 (PQ-only optional) |

---

## Abstract

ZBX Chain adopts NIST-standardized post-quantum cryptographic algorithms
(CRYSTALS-Dilithium for signatures, CRYSTALS-Kyber for key encapsulation)
alongside existing ECDSA/secp256k1 in a **hybrid dual-signature** scheme.
This ensures forward security against quantum adversaries while maintaining
full backward compatibility with existing wallets and tools.

---

## Motivation

Shor's algorithm running on a sufficiently powerful quantum computer can
break ECDSA (secp256k1) and BLS12-381 in polynomial time. While large-scale
quantum computers do not exist today, the "harvest now, decrypt later" threat
means adversaries can record today's signatures and break them once quantum
hardware matures (~2030–2040 estimates). Blockchain addresses are especially
vulnerable because public keys are exposed on-chain.

---

## Specification

### 1. Algorithm Selection

| Role               | Algorithm              | NIST Level | Key Size       | Sig Size    |
|--------------------|------------------------|------------|----------------|-------------|
| Transaction signing| CRYSTALS-Dilithium-3   | Level 3    | 1952 B pub     | 3293 B      |
| Legacy (compat)    | ECDSA secp256k1        | ~Level 1   | 33 B pub       | 64 B        |
| Key encapsulation  | CRYSTALS-Kyber-768     | Level 3    | 1184 B pub     | 1088 B ct   |
| P2P session keys   | Kyber-768 + X25519     | Level 3+   | hybrid         | hybrid      |

### 2. Hybrid Transaction Format (Block 500,000+)

```
TxV3 {
    // All existing EIP-1559 fields
    chain_id, nonce, max_priority_fee, max_fee, gas_limit,
    to, value, data, access_list, gas_token,

    // NEW: post-quantum signature (optional at first)
    pq_pub_key:  Option<DilithiumPubKey>,   // 1952 bytes
    pq_signature: Option<DilithiumSig>,     // 3293 bytes

    // EXISTING: ECDSA signature
    v, r, s,
}
```

A transaction is valid if:
- ECDSA signature is valid (always required pre-PQ-only activation), OR
- Dilithium signature is valid and `pq_pub_key` maps to sender address

### 3. PQ Address Derivation

```
pq_address = keccak256(dilithium_pub_key)[12..32]  // last 20 bytes
```

PQ addresses are indistinguishable from ECDSA addresses — no format change needed.

### 4. Key Derivation (Wallet Integration)

```
master_seed  (BIP-39 mnemonic, 256 bits)
    │
    ├─ secp256k1 key  = HKDF-SHA256(seed, "zbx-ecdsa-v1")
    └─ dilithium key  = HKDF-SHA256(seed, "zbx-dilithium-v1")
```

Both keys derived from same seed — one backup covers both.

### 5. P2P Encryption Upgrade

Existing noise protocol (XX handshake over X25519) is augmented:

```
XX_PQ handshake:
  → Kyber-768 encapsulation (post-quantum)
  → X25519 Diffie-Hellman (classical)
  → shared_key = HKDF(kyber_ss || x25519_ss)
```

This is a KEM-KEM hybrid — breaking it requires breaking BOTH.

### 6. Block Header Extension

```rust
pub struct BlockHeaderV2 {
    // ... all existing fields ...
    /// PQ aggregate signature over validators (Dilithium-aggregate).
    pub pq_validator_sig: Option<Vec<u8>>,
    /// PQ feature flag activation block.
    pub pq_epoch: Option<u64>,
}
```

### 7. Migration Phases

| Phase | Block      | Change                                              |
|-------|------------|-----------------------------------------------------|
| 0     | 0          | ECDSA only (current)                                |
| 1     | 500,000    | Hybrid: ECDSA required + Dilithium optional         |
| 2     | 750,000    | Hybrid: either signature accepted                   |
| 3     | TBD        | PQ-only (governance vote required)                  |

---

## Implementation

**New crate**: `zbx-pq`

```
zbx-pq/
├── src/
│   ├── lib.rs
│   ├── dilithium.rs    # CRYSTALS-Dilithium-3 signatures
│   ├── kyber.rs        # CRYSTALS-Kyber-768 KEM
│   ├── hybrid.rs       # Hybrid ECDSA + Dilithium tx signing
│   └── error.rs
```

---

## Security Analysis

- **Dilithium-3** provides NIST Security Level 3 (128-bit post-quantum security)
- Hybrid scheme: classical AND quantum adversary must both fail to forge
- No downgrade attack: hybrid requires valid classical sig until Phase 3
- Side-channel: Dilithium is constant-time; Kyber decapsulation is constant-time

---

## Backwards Compatibility

- Phase 1-2: all existing wallets continue to work unchanged
- Phase 3: governance vote required — at least 6 months notice
- EIP-1559 transaction format unchanged; pq fields are optional extensions

---

## References

- NIST FIPS 204 (CRYSTALS-Dilithium): https://csrc.nist.gov/pubs/fips/204/final
- NIST FIPS 203 (CRYSTALS-Kyber): https://csrc.nist.gov/pubs/fips/203/final
- Hybrid PQ signatures: IETF draft-ietf-pquip-hybrid-signature-spectrums
