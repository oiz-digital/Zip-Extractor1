//! Benchmark: cryptographic operations.

use criterion::{criterion_group, criterion_main, Criterion, Throughput};

fn bench_keccak256(c: &mut Criterion) {
    let mut group = c.benchmark_group("keccak256");
    group.throughput(Throughput::Bytes(1024));

    let data = vec![0u8; 1024];
    group.bench_function("1kb", |b| b.iter(|| {
        zbx_crypto::keccak::keccak256(&data)
    }));

    let data_32 = vec![0u8; 32];
    group.bench_function("32b", |b| b.iter(|| {
        zbx_crypto::keccak::keccak256(&data_32)
    }));

    group.finish();
}

fn bench_rlp_encode(c: &mut Criterion) {
    let mut group = c.benchmark_group("rlp");

    let short_list: Vec<Vec<u8>> = (0u8..10).map(|i| vec![i; 32]).collect();
    group.bench_function("encode_list_10x32", |b| b.iter(|| {
        zbx_rlp::encode_list(&short_list)
    }));

    group.finish();
}

criterion_group!(benches, bench_keccak256, bench_rlp_encode);
criterion_main!(benches);