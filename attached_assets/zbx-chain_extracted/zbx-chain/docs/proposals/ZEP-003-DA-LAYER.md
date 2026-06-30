# ZEP-003: DA Layer — Native Data Availability for Rollups

| Field         | Value                                       |
|---------------|---------------------------------------------|
| **ZEP**       | 003                                         |
| **Title**     | DA Layer — Native Data Availability Layer   |
| **Author**    | Zebvix Core Team                            |
| **Status**    | ACCEPTED                                    |
| **Category**  | Core                                        |
| **Activation**| Block 75,000                                |
| **Blob size** | 128 KB per blob                             |
| **Max blobs** | 8 per block (1 MB total)                    |

---

## Summary

ZEP-003 ZBX Chain mein **native Data Availability layer** add karta hai.

Rollups (L2 chains) apna transaction data ZBX pe publish kar sakte hain cheap blob transactions ke zariye — execution data store karne ki zaroorat nahi.

---

## Blob Transactions (Type 0x03)

```
Normal Tx (Type 0x02):  execution data on-chain stored karo
Blob Tx  (Type 0x03):  data sirf 30 days ke liye available, phir prune
```

Blobs ka fayda:
- Rollup batch data store karna **10x sasta** hota hai
- ZBX chain pe rollups host kar sako
- EIP-4844 compatible (same format as Ethereum)

---

## KZG Commitments

Data availability prove karne ke liye:
```
Blob Data → KZG Polynomial Commitment (48 bytes)
         → KZG Proof (48 bytes)
         → Versioned Hash (32 bytes, 0x01 prefix)
```

Light clients 75 random samples check karke 99.99% certainty se DA confirm kar sakte hain.

---

## Blob Fee Market

Execution gas se alag, blobs ki apni fee market:
```
blob_base_fee = f(blobs_used vs target_blobs)
Target: 4 blobs/block
Max:    8 blobs/block
```

---

## Crate

- `crates/zbx-da/` — 8 files
- `k8s/da-node.yaml` — dedicated DA node deployment
- `proto/da.proto` — gRPC interface