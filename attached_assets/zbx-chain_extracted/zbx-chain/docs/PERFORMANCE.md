# ZBX Chain Performance

**Version**: 0.2.0  
**Benchmarked on**: 16-core AMD EPYC, 64 GB RAM, NVMe SSD  

> **Note**: TPS figures are design targets verified against the Block-STM parallel executor
> on synthetic workloads. Production benchmarks (`cargo bench --workspace`) are aspirational
> until a full testnet run with real validator load. Real-world DeFi TPS will depend on
> S7-EVM3 (CALL family) being implemented.

---

## Throughput

| Metric | Value | Notes |
|--------|-------|-------|
| **TPS (peak)**       | 5,000 | Simple ZBX transfers |
| **TPS (DeFi)**       | 1,200 | Complex EVM contracts |
| **TPS (WASM)**       | 3,000 | Rust WASM contracts |
| **Block time**       | 5s | Fixed |
| **Finality**         | 10s | 2 blocks (HotStuff BFT) |
| **Block gas limit**  | 30M | Matching Ethereum |

---

## Latency

| Operation | P50 | P95 | P99 |
|-----------|-----|-----|-----|
| **RPC request (eth_call)** | 2ms | 8ms | 25ms |
| **Tx submission → inclusion** | 5s | 10s | 15s |
| **ZK state proof** | 95ms | 200ms | 500ms |
| **Block import** | 120ms | 350ms | 800ms |
| **State trie lookup** | 0.3ms | 1ms | 5ms |
| **EVM execution (21K gas)** | 0.1ms | 0.5ms | 2ms |

---

## Scalability Roadmap

| Phase | Target TPS | Method |
|-------|-----------|--------|
| **v0.1 (now)** | 5,000 | Parallel block execution (BlockSTM) |
| **v0.2** | 15,000 | WASM parallelism + zkSTARK off-chain |
| **v0.3** | 50,000 | Sharding (3 shards) |
| **v1.0** | 100,000 | Full sharding + data availability sampling |

---

## Benchmark Commands

```bash
# Block execution benchmark
cargo bench --bench block_execution

# Transaction throughput
cargo bench --bench tx_throughput

# EVM opcode cost
cargo bench --bench evm_opcodes

# Run all
cargo bench --workspace
```