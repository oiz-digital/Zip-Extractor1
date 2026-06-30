# ZBX Chain ZK Proof System (v0.2)

**Crate**: `zbx-prover`  
**Proof scheme**: STARK (FRI-based, no trusted setup)  
**Security**: 128-bit (conjectured)  
**Language**: Rust

---

## Overview

Zebvix Chain v0.2 introduces a ZK proof system enabling:

| Feature | What it enables |
|---------|-----------------|
| **State proofs** | Light clients (mobile wallets) verify balances without full node |
| **Block proofs** | Prove entire block execution is correct |
| **Fraud proofs** | Dispute bridge transactions / block producers |
| **Recursive proofs** | Compress 1000 block proofs into one ~48 KB proof |

---

## Architecture

```
                    zbx-prover crate
┌───────────────────────────────────────────────────────┐
│                                                       │
│  BlockWitness → Circuit (AIR) → STARK Prover → Proof  │
│                                                       │
│  Modules:                                             │
│    field.rs       — Goldilocks field arithmetic       │
│    circuit.rs     — AIR constraint system             │
│    witness.rs     — Execution trace generation        │
│    transcript.rs  — Fiat-Shamir (non-interactive)     │
│    prover.rs      — FRI-STARK proof generation        │
│    verifier.rs    — Proof verification                │
│    state_proof.rs — Light client state proofs         │
│    fraud_proof.rs — Bridge / block dispute proofs     │
│    recursive.rs   — Aggregate N proofs into one       │
│    params.rs      — Security parameters               │
└───────────────────────────────────────────────────────┘
         │ Proof (serialised)
         ▼
   ZbxVerifier.sol  (on-chain verification)
```

---

## Proof Types

### 1. State Proof (~1 KB)
Prove account balance/nonce without running a full node.

```rust
use zbx_prover::{StateProof, StateProofRequest};

let request = StateProofRequest {
    address:      [0xAB; 20],
    storage_keys: vec![],  // account proof only
    block_number: 10_000,
};
let proof = node.get_state_proof(request)?;
// Light client verifies:
proof.verify_account(&known_state_root)?;
println!("Balance: {} ZBX", proof.balance as f64 / 1e18);
```

### 2. Block Proof (~320 KB)
Prove full block execution is correct.

```rust
use zbx_prover::{Prover, BlockWitness, Circuit};

let witness  = witness_generator.generate_block_witness(...)?;
let circuit  = Circuit::state_transition();
let proof    = Prover::new().prove_block(&witness, &circuit)?;
// Verifier (another node / bridge contract):
Verifier::new().verify(&proof, block_number, &state_root_pre, &state_root_post)?;
```

### 3. Recursive Proof (~48 KB for 1000 blocks)
Aggregate multiple block proofs.

```rust
use zbx_prover::RecursiveProof;

let block_proofs: Vec<_> = (0..1000).map(|i| generate_block_proof(i)).collect();
let recursive = RecursiveProof::aggregate(&block_proofs)?;
// Submit ONE proof to Ethereum bridge contract instead of 1000:
bridge_contract.submit_recursive_proof(recursive.to_bytes());
```

---

## Proving Performance (Estimated)

| Proof type | Prove time | Verify time | Size |
|------------|-----------|-------------|------|
| State proof | <100 ms | <1 ms | ~1 KB |
| Block proof (1000 txs) | ~2 min (single core) / ~15s (GPU) | ~50 ms | ~320 KB |
| Recursive (100 blocks) | ~10 min | ~5 ms | ~48 KB |
| Fraud proof | ~3 min | ~20 ms | ~128 KB |

---

## Security

- **Proof scheme**: FRI-STARK (based on Plonky2 / ethSTARK)
- **Field**: Goldilocks (p = 2^64 - 2^32 + 1)
- **Hash function**: Keccak-256 (Fiat-Shamir transcript)
- **Security level**: 128 bits (conjecture: forging requires 2^128 work)
- **No trusted setup**: Unlike Groth16/PLONK, STARK requires no ceremony

---

## On-chain Verification (`ZbxVerifier.sol`)

The `ZbxVerifier` contract is deployed on Zebvix Chain and on Ethereum (for bridge security).

```solidity
// Verify account balance in a DeFi contract:
bool ok = zbxVerifier.verifyAccountState(
    account, balance, nonce,
    blockNumber, knownStateRoot,
    proof
);

// Verify 1000 blocks for bridge:
zbxVerifier.verifyRecursiveProof(
    firstBlock, lastBlock,
    stateRootPre, stateRootPost,
    1000, recursiveProof
);
```