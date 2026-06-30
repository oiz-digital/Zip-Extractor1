# ZEP-022: HotStuff-2 / Jolteon Consensus Upgrade

| Field       | Value                                        |
|-------------|----------------------------------------------|
| ZEP         | 022                                          |
| Title       | HotStuff-2 / Jolteon Consensus Upgrade       |
| Author      | Zebvix Core Team                             |
| Status      | ACCEPTED                                     |
| Category    | Core                                         |
| Created     | 2026-05-05                                   |
| Activation  | Block 400,000                                |

---

## Abstract

ZBX Chain upgrades its consensus from HotStuff-BFT (3-phase: Prepare →
PreCommit → Commit) to **HotStuff-2** (2-phase pipeline) combined with
**Jolteon** leader change. This reduces finality latency from 3 rounds to
2 rounds (33% faster), uses linear O(n) message complexity for leader
change (vs O(n²) in classic PBFT-style view change), and adds **optimistic
responsiveness** — blocks commit as fast as the network allows, not just
at the timeout.

---

## Motivation

Current HotStuff-BFT protocol:
- 3 rounds per block → 3 × network latency for finality
- View change: O(n) messages but high constant factor
- No responsiveness: always waits for full timeout even if all votes arrive early

HotStuff-2 improvements:
- **2-phase** commit: same BFT safety, one fewer round
- **Optimistic responsiveness**: commit the moment 2f+1 votes arrive
- **Jolteon** view change: O(n) messages, lower latency than original HotStuff

---

## Specification

### 1. HotStuff-2 Protocol

HotStuff-2 collapses the 3-phase protocol into 2 phases by observing that
consecutive QCs at the same round serve as implicit commit certificates:

```
Round r:
  Proposer:  broadcast PROPOSAL(b, round=r, justify=QC(r-1))
  Validators: vote(b) → VOTE(b, r)
  Proposer:  collect 2f+1 votes → form QC(r)

Round r+1:
  Proposer:  broadcast PROPOSAL(b', round=r+1, justify=QC(r))
  Validators: seeing QC(r) in PROPOSAL(b', r+1) →
              COMMIT block b (from round r) ← TWO-PHASE COMMIT
              vote(b') → VOTE(b', r+1)
```

**Safety**: two consecutive QCs at round r, r+1 → block at r is committed
**Liveness**: same 2f+1 threshold as HotStuff

### 2. Optimistic Responsiveness

```rust
pub struct RoundTimer {
    /// Minimum wait before declaring timeout (network delay estimate)
    pub delta_min: Duration,
    /// Maximum wait before forcing timeout
    pub delta_max: Duration,
    /// Current adaptive estimate
    pub delta_current: Duration,
}

impl RoundTimer {
    /// Called when 2f+1 votes received — commit immediately, don't wait for timeout
    pub fn on_quorum_reached(&mut self) -> CommitDecision;
    /// Called when timeout fires — update delta upward
    pub fn on_timeout(&mut self) -> TimeoutDecision;
    /// Update delta based on observed round latency
    pub fn update_delta(&mut self, observed_latency: Duration);
}
```

Block time adapts: if network is fast, blocks come faster than 5s target.
If network is slow, blocks slow down gracefully rather than failing.

### 3. Jolteon View Change (Leader Change)

When a leader fails or is Byzantine, validators trigger a view change:

```
Timeout(r):
  1. Each validator broadcasts TC_SHARE(r, highest_qc)
  2. Proposer of r+1 collects 2f+1 TC_SHARE messages
  3. Aggregates into TIMEOUT_CERTIFICATE(r)
  4. Next leader uses TC(r) to advance past round r

TC_SHARE message:
  { validator, round: r, highest_qc_round: q, sig: BLS_Sign(r || q) }

TIMEOUT_CERTIFICATE:
  { round: r, highest_qc_round: q, agg_sig: BLS_Agg([sigs...]), bitmap }
```

Key property: Jolteon requires only **2f+1 TC_SHAREs** and 1 BLS aggregate —
O(n) total messages, same as normal rounds. No O(n²) all-to-all communication.

### 4. Pipelining

HotStuff-2 naturally pipelines: while validators vote on round r+1, the
proposer is already preparing round r+2:

```
Round:  r      r+1     r+2     r+3
        │      │       │       │
Propose b₁    b₂      b₃      b₄
Vote    b₁    b₂      b₃      b₄
Commit         b₁      b₂      b₃
```

Throughput = 1 block committed per round (vs 1 block per 3 rounds in naive HotStuff).

### 5. State Machine

```rust
pub enum HotStuff2Phase {
    /// Waiting for proposal from leader
    WaitingProposal { round: u64 },
    /// Voted, waiting for next proposal with QC to trigger commit
    Voted { round: u64, voted_block: H256 },
    /// Committed block
    Committed { block: Block, qc: QuorumCertificate },
    /// Round timed out — participating in Jolteon view change
    ViewChange { round: u64, tc_shares: Vec<TimeoutShare> },
}

pub struct HotStuff2 {
    pub phase:          HotStuff2Phase,
    pub highest_qc:     QuorumCertificate,
    pub round_timer:    RoundTimer,
    pub vote_accum:     VoteAccumulator,
    pub tc_accum:       TcAccumulator,
}
```

### 6. Safety Rules (unchanged from HotStuff)

```rust
pub struct SafetyRules {
    /// Never vote for two different blocks at same round
    voted_rounds: HashMap<u64, H256>,
    /// Only vote if proposed block extends highest committed chain
    highest_commit_round: u64,
}
```

Safety rules remain identical to HotStuff — the upgrade is backward-compatible
in terms of safety guarantees.

---

## Implementation

**Crate**: `zbx-consensus` — new module `hotstuff2.rs`

```
zbx-consensus/src/
├── hotstuff.rs    # existing (kept for reference/migration)
├── hotstuff2.rs   # NEW: HotStuff-2 + Jolteon
├── liveness.rs    # UPGRADED: Jolteon timeout certificate
├── vote.rs        # UPGRADED: BLS aggregate votes (from ZEP-016)
```

---

## Performance Comparison

| Metric               | HotStuff (current)   | HotStuff-2 (ZEP-022) |
|----------------------|----------------------|----------------------|
| Rounds to commit     | 3                    | 2                    |
| View change messages | O(n)                 | O(n)                 |
| Responsiveness       | Fixed timeout        | Adaptive (network)   |
| Block time (fast net)| 5s (timeout-bound)   | 2-3s (vote-bound)    |
| Block time (slow net)| 5s                   | 5s (falls back)      |

---

## References

- HotStuff-2: https://eprint.iacr.org/2023/397
- Jolteon: https://arxiv.org/abs/2106.10362
- Original HotStuff: https://arxiv.org/abs/1803.05069
