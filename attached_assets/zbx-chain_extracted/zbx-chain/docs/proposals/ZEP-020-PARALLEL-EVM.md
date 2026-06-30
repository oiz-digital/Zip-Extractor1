# ZEP-020: Parallel EVM Execution (Block-STM)

| Field       | Value                                        |
|-------------|----------------------------------------------|
| ZEP         | 020                                          |
| Title       | Parallel EVM Execution — Block-STM           |
| Author      | Zebvix Core Team                             |
| Status      | ACCEPTED                                     |
| Category    | Core                                         |
| Created     | 2026-05-05                                   |
| Activation  | Block 250,000                                |

---

## Abstract

ZBX Chain replaces sequential transaction execution with **Block-STM**
(Software Transactional Memory for Blockchains), enabling parallel transaction
execution across multiple CPU cores. Transactions that access disjoint state
execute concurrently; conflicting transactions are re-executed sequentially.
This delivers 4-20x throughput improvement on commodity hardware with
no changes to the EVM semantics or user-facing transaction format.

---

## Motivation

Ethereum executes transactions sequentially — one at a time. On ZBX Chain
with 150M gas blocks and 5s block time, sequential execution is the primary
throughput bottleneck. Modern servers have 32-128 cores sitting idle during
block processing.

Block-STM (developed by Aptos, also used by Polygon, Monad) enables:
- Fully parallel execution of non-conflicting transactions
- Correct EVM semantics preserved (same outputs as sequential execution)
- No changes needed to existing smart contracts or wallets
- 4-20x throughput improvement depending on access pattern diversity

---

## Specification

### 1. Block-STM Algorithm

```
Phase 1: OPTIMISTIC EXECUTION (parallel)
  ├── Spawn N worker threads (N = CPU cores - 2)
  ├── Each thread picks a tx from the schedule queue
  ├── Execute tx against a multi-version data structure (MVDB)
  │   └── Reads: get latest version written by committed tx with lower idx
  │   └── Writes: create new version in MVDB (not committed yet)
  └── Track all read/write sets per tx

Phase 2: VALIDATION (parallel)
  ├── For each tx, validate its read set:
  │   └── Did any lower-index committed tx write to a location we read?
  │   └── If YES → abort and re-execute this tx
  ├── Valid tx → mark committed
  └── Aborted tx → re-schedule for re-execution

Phase 3: COMMIT
  ├── All txs validated and committed
  ├── Apply final MVDB state to world state
  └── Produce receipts
```

### 2. Multi-Version Data Structure (MVDB)

```rust
pub struct MvMemory {
    /// data[location] = sorted Vec<(tx_idx, TxVersion, WriteKind)>
    data: DashMap<StorageKey, BTreeMap<TxIdx, MvEntry>>,
}

pub enum MvEntry {
    Write(Bytes),       // value written by tx_idx
    ReadEstimate,       // placeholder during optimistic execution
    Deleted,            // tx deleted this key
}

pub enum ReadResult {
    Value(Bytes),       // read from committed lower-index write
    Uninitialized,      // must read from base storage
    Estimate(TxIdx),    // read estimate → dependency (re-validate if changes)
}
```

### 3. Access Set Tracking

```rust
pub struct AccessSet {
    pub reads:  Vec<StorageKey>,   // all keys read during execution
    pub writes: Vec<StorageKey>,   // all keys written during execution
}

pub struct StorageKey {
    pub address: Address,
    pub slot:    H256,             // H256::zero() for account fields
}
```

### 4. Execution Scheduler

```rust
pub struct BlockStmScheduler {
    /// Transactions remaining to execute
    execution_queue: BTreeMap<TxIdx, ()>,
    /// Transactions in validation phase
    validation_queue: BTreeMap<TxIdx, ()>,
    /// Committed transaction count
    committed: usize,
    /// Total transactions in block
    total: usize,
}

impl BlockStmScheduler {
    pub fn next_task(&self) -> Option<SchedulerTask>;
    pub fn finish_execution(&mut self, idx: TxIdx, writes: AccessSet);
    pub fn finish_validation(&mut self, idx: TxIdx, valid: bool);
}

pub enum SchedulerTask {
    Execute(TxIdx),
    Validate(TxIdx),
    Done,
}
```

### 5. Conflict Analysis

Conflict types handled:
- **Read-After-Write (RAW)**: tx B reads slot X that tx A (lower index) wrote → dependency
- **Write-After-Write (WAW)**: both tx A and B write slot X → only later idx survives
- **Anti-dependence**: safe in STM (each tx has own MVDB version)

For typical DeFi blocks:
- ~70% of txs access disjoint state → fully parallel
- ~20% share token contracts → mild contention
- ~10% same pool/account → sequential re-execution

Expected speedup: 6-10x on 16-core server.

### 6. Determinism Guarantee

Block-STM always produces **identical output** to sequential execution:
- Same state root
- Same receipt root
- Same gas usage per tx
- Deterministic regardless of scheduling order

This is mathematically guaranteed by the STM validation phase.

### 7. Configuration

```rust
pub struct ParallelExecConfig {
    pub num_workers: usize,          // default: num_cpus - 2
    pub max_retries: usize,          // max re-executions per tx (default: 10)
    pub sequential_fallback: bool,   // fall back to sequential on error
    pub conflict_threshold: f64,     // switch to sequential if conflict rate > X%
}
```

---

## Implementation

**Crate**: `zbx-execution` (already has `parallel.rs` module — upgrading)

```
zbx-execution/src/
├── parallel.rs       # UPGRADED: full Block-STM implementation
├── scheduler.rs      # UPGRADED: Block-STM scheduler
├── executor.rs       # UPGRADED: use parallel by default
```

---

## Performance Benchmarks (Target)

| Metric                        | Sequential | Block-STM (16 cores) |
|-------------------------------|------------|----------------------|
| Simple transfers              | 1,000 TPS  | 8,000 TPS            |
| ERC-20 transfers (1 token)    | 800 TPS    | 2,000 TPS            |
| DEX swaps (multiple pools)    | 500 TPS    | 3,000 TPS            |
| Mixed workload                | 700 TPS    | 5,000 TPS            |

---

## References

- Block-STM paper: https://arxiv.org/abs/2203.06871 (Aptos/Meta)
- Polygon Block-STM: https://polygon.technology/blog/block-stm
- Monad parallel EVM: https://www.monad.xyz/
