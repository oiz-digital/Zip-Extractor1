# HotStuff BFT Consensus

Zebvix Chain uses a production implementation of **HotStuff BFT** — the same
consensus family used by Aptos, Diem, and Jolteon.

## Properties

| Property           | Value               |
|--------------------|---------------------|
| Safety threshold   | f < n/3 (Byzantine) |
| Liveness threshold | f < n/3             |
| Message complexity | O(n) per round      |
| View change        | Linear (O(n))       |
| Finality           | 2-round (direct)    |
| Block time         | 5 seconds           |

## Protocol Phases

### Phase 1 — Proposal
The leader for view `v` broadcasts a `Proposal` message containing:
- The block (header + tx list)
- A `HighQC` from the previous round (justifying the block's parent)

### Phase 2 — Vote
Each validator verifies the proposal and sends a BLS partial signature (`Vote`)
to the next leader. Votes are aggregated into a **Quorum Certificate (QC)**.

### Phase 3 — Commit
A block is **committed** (finalized) when its QC is included in a subsequent
block's `HighQC` chain of length 2 (direct-commit rule):

```
B_n  ←  B_{n+1} (QC for B_n)  →  commit B_n
```

## View Change (Pacemaker)

If a validator does not receive a valid proposal within `round_timeout_ms`,
it broadcasts a `Timeout` message. Once 2f+1 timeouts are collected for
view `v`, a `TimeoutCertificate (TC)` is formed and the view advances to `v+1`.

## Signatures

All consensus messages use **BLS12-381** aggregate signatures:
- Private key: 32-byte scalar on BLS12-381 Fr
- Public key:  G1 point (48 bytes compressed)
- Signature:   G2 point (96 bytes compressed)
- Aggregation: `agg_sig = sig_1 + sig_2 + ... + sig_k` (additive on G2)

## Safety Rule

A validator only votes on proposal `B` if:
1. `B.parent` matches its `lockedBlock` OR `B.height > lockedBlock.height`
2. `B`'s `HighQC` is at least as high as its own `HighQC`

## Liveness Rule

If a validator cannot progress (no valid proposal in timeout), it sends a
`Timeout` to the next leader and increments its local view.