#!/usr/bin/env bash
# Source this file before running cargo commands in the Replit sandbox.
# Usage:  source scripts/cargo-env.sh && cargo check --workspace
#
# Points librocksdb-sys + bindgen at system libraries from the Nix store
# instead of compiling RocksDB from source (which OOMs the sandbox).

export LIBCLANG_PATH=/nix/store/0cyla3kp9qdq9x64lr6q1fd9my54cm9w-clang-17.0.6-lib/lib
export ROCKSDB_LIB_DIR=/nix/store/06qwd199wjzxgzp5m3kr1jr80cb8ppzr-rocksdb-8.3.2/lib
export ROCKSDB_INCLUDE_DIR=/nix/store/06qwd199wjzxgzp5m3kr1jr80cb8ppzr-rocksdb-8.3.2/include
export ROCKSDB_STATIC=0

# SEC-2026-05-09 Pass-11 — system rocksdb 8.3.2 was built with
# liburing support; binaries linking rocksdb need to find liburing
# at link-time. Surfaces only when a crate produces an actual
# binary (e.g. `cargo test -p zbx-staking` builds a test runner;
# plain `cargo check -p zbx-storage` does not).
export LIBURING_LIB_DIR=/nix/store/0ca48diyyzpwg8lvgdvry73mnj5mf1bf-liburing-2.3/lib
export RUSTFLAGS="${RUSTFLAGS:-} -L native=${LIBURING_LIB_DIR} -l uring"
