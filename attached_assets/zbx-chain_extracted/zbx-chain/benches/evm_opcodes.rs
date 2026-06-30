//! Benchmark: EVM opcode execution throughput.

use criterion::{criterion_group, criterion_main, Criterion, BenchmarkId, Throughput};
use zbx_vm::{Evm, EvmConfig};
use zbx_vm::stack::Stack;
use zbx_vm::memory::Memory;
use zbx_types::U256;

fn bench_stack_push_pop(c: &mut Criterion) {
    let mut group = c.benchmark_group("stack");
    group.throughput(Throughput::Elements(1000));

    group.bench_function("push_pop_1000", |b| b.iter(|| {
        let mut stack = Stack::new();
        for i in 0..1000u64 {
            stack.push(U256::from(i)).unwrap();
        }
        for _ in 0..1000 {
            stack.pop().unwrap();
        }
    }));

    group.finish();
}

fn bench_memory_expansion(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory");

    group.bench_function("expand_to_1mb", |b| b.iter(|| {
        let mut mem = Memory::new();
        let _ = mem.expansion_gas(0, 1024 * 1024);
    }));

    group.bench_function("write_read_u256", |b| b.iter(|| {
        let mut mem = Memory::new();
        for i in 0..32usize {
            mem.set_u256(i * 32, U256::from(i as u64));
        }
        for i in 0..32usize {
            let _ = mem.get_u256(i * 32);
        }
    }));

    group.finish();
}

criterion_group!(benches, bench_stack_push_pop, bench_memory_expansion);
criterion_main!(benches);