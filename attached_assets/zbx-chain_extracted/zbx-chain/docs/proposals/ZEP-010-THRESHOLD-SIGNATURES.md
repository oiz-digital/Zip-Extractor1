# ZEP-010: FROST Threshold Signatures

| Field      | Value                                       |
|:---|:---|
| ZEP Number | ZEP-010                                     |
| Title      | FROST Threshold Signatures                  |
| Status     | **Draft** — targets block 200,000           |
| Category   | Core / Cryptography                         |
| Authors    | Zebvix Core Team                            |

## Abstract

ZEP-010 replaces single-key validator signing with FROST (Flexible Round-Optimized
Schnorr Threshold Signatures). Validators form committees and sign blocks using
t-of-n threshold signing, eliminating single points of failure.

## Motivation

Single validator keys are vulnerable to:
- Node compromise (one key leaks → attacker signs anything)
- Validator downtime (one crash → missed signatures)
- Key management errors (lost key → validator slashed)

FROST distributes the signing key across n validators. Even if t-1 are compromised
or offline, signatures are impossible. Requires ≥t validators to collaborate.

## Scheme: FROST-Schnorr

| Property           | Value                                    |
|:---|:---|
| Base curve         | Secp256k1 (same as ZBX accounts)        |
| Signature scheme   | Schnorr (BIP-340 compatible)            |
| Rounds             | 2 (can be done asynchronously)           |
| Signature size     | 65 bytes (same as ECDSA)                |
| Security           | 128-bit (DL assumption)                 |
| Threshold          | t = ⌈2n/3⌉ + 1 (BFT majority)          |

## Key Generation (DKG)

1. Each validator samples a random polynomial of degree t-1
2. Coefficients are broadcast as Feldman VSS commitments
3. Each validator receives a share from every other validator
4. Group public key is derived from the VSS commitments
5. Each validator verifies their share is consistent with the group key

## Signing Flow

```
Round 1 (broadcast):  Each signer → (D_i, E_i) nonce commitments
Round 2 (broadcast):  Each signer → z_i partial signature
Aggregation:          Combiner sums partial sigs → (R, s) threshold sig
Verification:         Anyone verifies (R, s) against group_key and message
```

## Integration with ZBX Consensus

- Block proposal: Proposer collects Round 1 nonces from committee
- After block: Committee members produce Round 2 partial sigs
- Aggregator (any node): Combines into one Schnorr sig appended to block
- Light clients: Verify one Schnorr sig (don't need all committee keys)