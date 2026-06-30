# WASM Smart Contracts on ZBX Chain

**Version**: 0.2  
**Crate**: zbx-wasm  
**Status**: Testnet

---

## Overview

ZBX Chain supports two smart contract runtimes side-by-side:

| Feature | EVM (Solidity) | WASM (Rust/C++) |
|---------|---------------|-----------------|
| Language | Solidity, Vyper | Rust, C, Go, AssemblyScript |
| Tooling | Hardhat, Foundry | cargo, wasm-pack |
| Gas model | EVM opcodes | WASM instructions (fuel) |
| Contract size | 24 KB max | 4 MB max |
| Determinism | ✓ | ✓ (threads disabled) |
| ZK-friendliness | Good | Better (Goldilocks-native) |
| Memory model | Stack + heap (EVM) | Linear memory (65 KB pages) |

---

## Writing a WASM Contract

### 1. Setup

```bash
cargo new --lib my-contract
cd my-contract
```

### 2. Cargo.toml

```toml
[package]
name = "my-contract"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]   # compile to WASM

[dependencies]
zbx-contract-sdk = "0.1"  # ZBX WASM SDK
```

### 3. Contract code

```rust
use zbx_contract_sdk::prelude::*;

#[zbx_contract]
pub mod counter {
    #[storage]
    static COUNT: u64 = 0;

    #[callable]
    pub fn increment() {
        COUNT.set(COUNT.get() + 1);
        emit("Incremented", COUNT.get());
    }

    #[view]
    pub fn get_count() -> u64 {
        COUNT.get()
    }
}
```

### 4. Compile and deploy

```bash
cargo build --target wasm32-unknown-unknown --release
zbx contract deploy target/wasm32-unknown-unknown/release/my_contract.wasm
```

---

## Host API Reference

| Function | Description |
|----------|-------------|
| `zbx_storage_get(key)` | Read contract storage |
| `zbx_storage_set(key, val)` | Write contract storage |
| `zbx_transfer(to, amount)` | Transfer ZBX |
| `zbx_balance(addr)` | Read ZBX balance |
| `zbx_call(to, data)` | Call another contract |
| `zbx_emit(topics, data)` | Emit event |
| `zbx_keccak256(data)` | Compute Keccak hash |

---

## Gas Model

WASM contracts use "fuel" units (1 ZBX gas ≈ 10 WASM instructions):

```
max_gas = 10,000,000 (default)
fuel    = max_gas / 10 = 1,000,000 Wasmtime fuel units
```