# ZEP-023: Enhanced Validator Slashing

| Field       | Value                                        |
|-------------|----------------------------------------------|
| ZEP         | 023                                          |
| Title       | Enhanced Validator Slashing v2               |
| Author      | Zebvix Core Team                             |
| Status      | ACCEPTED                                     |
| Category    | Core                                         |
| Created     | 2026-05-05                                   |
| Activation  | Block 200,000                                |

---

## Abstract

ZBX Chain upgrades validator slashing with: (1) **on-chain slashing evidence**
storage for auditability, (2) **optimistic slashing with appeal window** to
catch lazy/malicious validators while giving honest validators 24h to contest,
(3) **correlated slashing** that scales slash amount with how many validators
misbehave simultaneously (up to 100% for coordinated attacks), and
(4) **whistleblower rewards** for submitting valid slash evidence.

---

## Motivation

Current slashing (v1):
- Basic double-sign (5%) and liveness fault detection
- No on-chain evidence → hard to audit
- No appeal mechanism → honest validators punished for bugs
- Fixed slash % regardless of attack scale

Enhanced slashing (v2):
- Evidence stored on-chain → transparent, verifiable, auditable
- 24h appeal window → protection for honest validators
- Correlated slashing → stronger deterrent for coordinated attacks
- Whistleblower incentive → crowdsourced detection

---

## Specification

### 1. On-Chain Evidence Storage

```rust
pub struct SlashEvidenceRecord {
    pub id:            H256,         // keccak256(evidence bytes)
    pub evidence_type: EvidenceType,
    pub offender:      Address,
    pub submitted_by:  Address,      // whistleblower
    pub submit_block:  u64,
    pub evidence:      SlashEvidence,
    pub status:        EvidenceStatus,
    pub slash_amount:  u128,
    pub appeal_deadline: u64,        // submit_block + APPEAL_WINDOW
}

pub enum EvidenceType {
    DoubleSign,
    LivenessFault,
    ConsecutiveMiss,
    SurrogateVote,     // NEW: signing on behalf of non-existent validator
    InvalidBlock,      // NEW: proposing block that violates protocol rules
}

pub enum EvidenceStatus {
    Pending,           // appeal window open
    Appealed,          // under appeal review
    Confirmed,         // slash applied
    Rejected,          // evidence invalid (whistleblower penalized)
    Overturned,        // appeal succeeded (slash reversed)
}
```

### 2. Optimistic Slashing Flow

```
Block B: Evidence submitted → status = Pending
  └── Slash amount reserved from validator stake (not yet burned)

Block B + APPEAL_WINDOW (172,800 blocks ≈ 10 days):
  └── No appeal filed → status = Confirmed → slash_amount burned
  
Block B + N (N < APPEAL_WINDOW): Validator files appeal
  └── status = Appealed
  └── Governance fast-track vote (7 days)
    ├── Vote passes → Overturned → stake returned
    └── Vote fails  → Confirmed  → slash applied immediately
```

```rust
pub const APPEAL_WINDOW: u64 = 172_800; // ~10 days at 5s/block
```

### 3. Correlated Slashing

When multiple validators misbehave in the same epoch, slash severity scales:

```
correlated_slash_pct = base_slash × (1 + 3 × (N_slashed / N_total))²

where:
  base_slash   = 5% for double-sign, 0.01%/day for liveness
  N_slashed    = number of validators slashed this epoch
  N_total      = total validators

Examples:
  1 validator misbehaves:  5% × (1 + 3×0.01)² ≈ 5.3%
  10% of validators:       5% × (1 + 3×0.10)² ≈ 8.45%
  33% of validators:       5% × (1 + 3×0.33)² ≈ 18.5%
  67%+ of validators:      100% (catastrophic failure)
```

This strongly deters coordinated attacks — a cartel trying to take over the
network faces total stake loss.

### 4. Whistleblower Rewards

```rust
pub struct WhistleblowerReward {
    pub evidence_id: H256,
    pub submitter:   Address,
    pub reward:      u128,    // 5% of slashed amount
}

pub const WHISTLEBLOWER_PCT: u128 = 500; // 5% in basis points
```

Evidence submission requires a small bond to prevent spam:

```rust
pub const EVIDENCE_BOND: u128 = 100 * 10u128.pow(18); // 100 ZBX
```

If evidence is valid: bond returned + 5% of slashed amount.
If evidence is invalid: bond slashed (to prevent frivolous submissions).

### 5. Double-Sign Proof Format

```rust
pub struct DoubleSignProof {
    pub height:     u64,
    pub round:      u64,
    pub phase:      u8,         // 0=Prepare, 1=PreCommit, 2=Commit
    pub block_a:    H256,       // first block signed
    pub block_b:    H256,       // conflicting block signed
    pub sig_a:      Signature,  // validator signature on block_a
    pub sig_b:      Signature,  // validator signature on block_b
    pub validator:  Address,    // claimed offender
}

impl DoubleSignProof {
    pub fn verify(&self, vk: &BlsPubKey) -> bool {
        // Verify both signatures are valid BLS sigs from same validator
        // Verify they sign different blocks at same (height, round, phase)
        bls_verify(vk, &self.block_a, &self.sig_a)
            && bls_verify(vk, &self.block_b, &self.sig_b)
            && self.block_a != self.block_b
    }
}
```

### 6. Invalid Block Proof

```rust
pub struct InvalidBlockProof {
    pub block_hash: H256,
    pub proposer:   Address,
    pub violation:  BlockViolation,
    pub witness:    Vec<u8>,    // execution trace proving invalidity
}

pub enum BlockViolation {
    InvalidStateRoot { claimed: H256, actual: H256 },
    InvalidTxRoot,
    GasLimitExceeded,
    InvalidTimestamp,
    ChainIdMismatch,
}
```

---

## Implementation

**Crate**: `zbx-staking` — new module `slashing_v2.rs`

```
zbx-staking/src/
├── slashing.rs      # existing v1 (kept)
├── slashing_v2.rs   # NEW: on-chain evidence + appeal + correlated
├── validator.rs     # UPGRADED: slash_amount tracking per epoch
```

---

## Slash Amount Summary

| Offence                  | Base Slash | Correlated (33% attackers) |
|--------------------------|------------|----------------------------|
| Double-sign              | 5%         | 18.5%                      |
| Liveness fault           | 0.01%/day  | 0.037%/day                 |
| Consecutive miss (20 bl) | 1%         | 3.7%                       |
| Invalid block            | 20%        | Instant jail                |
| Coordinated (67%+)       | 100%       | 100%                        |

---

## References

- Ethereum slashing design: https://eth2book.info/capella/part2/incentives/slashing/
- Cosmos slashing parameters
- Correlated slashing (EIP): https://notes.ethereum.org/@adiasg/slashing-and-leaking
