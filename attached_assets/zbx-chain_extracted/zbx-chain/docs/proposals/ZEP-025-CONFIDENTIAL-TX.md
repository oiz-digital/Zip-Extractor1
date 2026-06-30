# ZEP-025: Confidential Transactions

| Field       | Value                                          |
|-------------|------------------------------------------------|
| ZEP         | 025                                            |
| Title       | Confidential Transactions (Optional Privacy)   |
| Author      | Zebvix Core Team                               |
| Status      | ACCEPTED                                       |
| Category    | Standard / Core                                |
| Created     | 2026-05-05                                     |
| Activation  | Block 600,000                                  |

---

## Abstract

ZBX Chain adds **optional confidential transactions** using Pedersen
commitments for amount hiding, stealth addresses for recipient privacy,
and Bulletproofs range proofs for validity without revealing amounts.
Confidential transactions are opt-in — regular transparent transactions
remain the default. ZBX (native gas token) always stays transparent;
confidential mode applies to ZRC-20 token transfers only.

---

## Motivation

All blockchain transactions are fully transparent by default — anyone can
see who sent what to whom. For many use cases (payroll, B2B payments,
healthcare, private DEX trading), this is unacceptable.

ZBX Chain takes the pragmatic approach: transparency by default (for
auditability and compliance), with opt-in privacy for those who need it.
This mirrors how TradFi works: public ledger for regulatory purposes,
with selective disclosure.

---

## Specification

### 1. Pedersen Commitments

A commitment hides a value without revealing it:

```
C = v·G + r·H

where:
  v = value (amount) — secret
  r = blinding factor — secret (random)
  G, H = public generator points on Ristretto255 (curve)
  C = commitment — public
```

Properties:
- **Hiding**: C reveals nothing about v (perfectly hiding with random r)
- **Binding**: Cannot open C to a different v' (computationally binding)
- **Homomorphic**: C(v₁) + C(v₂) = C(v₁ + v₂) — sum of commitments = commitment to sum

Homomorphism enables **balance verification without revealing amounts**:
```
Input commitments sum = Output commitments sum
→ No ZBX created or destroyed (conservation law)
```

### 2. Range Proofs (Bulletproofs)

A range proof proves 0 ≤ v < 2⁶⁴ without revealing v:

```rust
pub struct RangeProof {
    /// Compressed Bulletproof (logarithmic size)
    pub proof: Vec<u8>,      // ~700 bytes for 64-bit range
    pub commitment: PedersenCommit,
}

pub fn prove_range(value: u64, blinding: Scalar) -> RangeProof;
pub fn verify_range(proof: &RangeProof) -> bool;
```

Prevents negative value attacks (someone "sending" -1000 tokens to create 1000 out of thin air).

### 3. Stealth Addresses

Recipient posts a **stealth meta-address** publicly. Sender computes a one-time
address only the recipient can spend from:

```
Recipient publishes:
  (K_s, K_v) = (spend_pubkey, view_pubkey)

Sender:
  r = random scalar
  R = r·G           (ephemeral pubkey, included in tx)
  shared = ECDH(r, K_v) = r·K_v
  S = H(shared) + K_s  (stealth address pubkey)
  address = keccak256(S.to_bytes())[12..32]

Recipient scans all txs:
  For each tx with ephemeral R:
    shared = ECDH(v_k, R) = v_k·R
    S' = H(shared) + K_s
    If S' matches tx recipient → this tx is mine
    spend_key_for_tx = s_k + H(shared)
```

Sender → recipient transfer is unlinkable to outside observers.
Recipient can detect received funds using view_key (without spend capability).

### 4. Confidential Transaction Type

```rust
pub struct ConfidentialTx {
    /// Encrypted inputs (from sender's committed balances)
    pub inputs: Vec<ConfidentialInput>,
    /// Encrypted outputs
    pub outputs: Vec<ConfidentialOutput>,
    /// Proves sum(inputs) - sum(outputs) = fee (conservation)
    pub balance_proof: BalanceProof,
    /// Ephemeral pubkey for stealth address derivation
    pub ephemeral_key: [u8; 32],
    /// Gas fee (transparent — always in ZBX or ZUSD)
    pub fee: u128,
    /// Sender signature (over all commitments)
    pub signature: Signature,
}

pub struct ConfidentialOutput {
    /// Pedersen commitment to output amount
    pub commitment: PedersenCommit,
    /// Range proof: proves amount ∈ [0, 2⁶⁴)
    pub range_proof: RangeProof,
    /// Encrypted amount + blinding factor (for recipient to decrypt)
    pub encrypted_data: EncryptedNote,
    /// Stealth address (one-time address)
    pub stealth_address: Address,
}

pub struct EncryptedNote {
    /// ECIES-encrypted (v, r) pair for recipient
    pub ciphertext: Vec<u8>,  // 128 bytes
    pub nonce: [u8; 12],
}
```

### 5. Balance Conservation Proof

Proves inputs = outputs + fee without revealing individual values:

```
balance_proof proves:
  C(v_in₁) + C(v_in₂) + ... = C(v_out₁) + C(v_out₂) + ... + C(fee)
  
By homomorphism:
  C(v_in₁ + v_in₂ + ...) = C(v_out₁ + v_out₂ + ... + fee)
  → v_in_total = v_out_total + fee
  
This is verified by: C_inputs_sum - C_outputs_sum - C(fee) = C(0)?
```

### 6. Compliance: Selective Disclosure

Users can reveal transactions to auditors/regulators by sharing:
- Blinding factor r → reveals amount v (breaks hiding, proves value)
- View key K_v → allows scanning all received transactions
- Tx decryption key → reveals specific transaction details

This enables:
- Tax reporting without public disclosure
- Regulatory compliance on demand
- Audit trails for institutional users

### 7. Scope (What Is / Isn't Private)

| Element                    | Private?   | Notes                              |
|----------------------------|------------|------------------------------------|
| ZRC-20 token amount        | YES        | Pedersen commitment                |
| Recipient address          | YES        | Stealth address                    |
| Sender address             | YES (weak) | Can be inferred from gas payer     |
| ZBX native gas amount      | NO         | Always transparent                 |
| Transaction existence      | NO         | Still on-chain                     |
| Block height / timestamp   | NO         | Always transparent                 |

---

## Implementation

**New crate**: `zbx-confidential`

```
zbx-confidential/
├── Cargo.toml
└── src/
    ├── lib.rs
    ├── commitment.rs   # Pedersen commitments + homomorphic ops
    ├── stealth.rs      # Stealth addresses (ECDH-based)
    ├── range_proof.rs  # Bulletproofs range proofs
    ├── note.rs         # EncryptedNote for recipient
    ├── tx.rs           # ConfidentialTx type + validation
    └── error.rs
```

---

## Gas Costs

| Operation                  | Gas Cost   | Notes                    |
|----------------------------|------------|--------------------------|
| Range proof verify         | 50,000     | Per output               |
| Pedersen commitment        | 3,000      | Per operation            |
| Stealth address derive     | 5,000      | Per output               |
| Balance proof verify       | 15,000     | Per transaction          |
| Encrypted note decode      | 2,000      | Per output               |

---

## References

- Confidential Transactions: https://elementsproject.org/features/confidential-transactions
- Bulletproofs: https://eprint.iacr.org/2017/1066
- Stealth Addresses: ERC-5564 (Ethereum stealth addresses)
- Monero Ring CT: inspiration for amount hiding design
