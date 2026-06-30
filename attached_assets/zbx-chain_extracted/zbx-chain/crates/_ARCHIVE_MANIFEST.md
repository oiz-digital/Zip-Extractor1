# `crates/*/src/_archive/` — Intentional Module Backlog

_Generated: 2026-05-01_

## What this is

Each Rust file under `crates/<crate>/src/_archive/` is a **planned future
module** — code written during architectural exploration but not yet wired
into its parent crate's `lib.rs`. Cargo ignores files in subdirectories
unless declared via `mod`, so these files do NOT participate in any build,
do NOT pull in dependencies, and do NOT affect runtime behaviour. They are
preserved here as a design backlog rather than deleted, so future sessions
can rewire them deliberately rather than re-implementing from scratch.

## Lifecycle policy

1. **Rewire**: when a session is ready to ship one of these modules, move
   it back to `crates/<crate>/src/<file>.rs` and add `pub mod <file>;` to
   `lib.rs`. Run `cargo check -p <crate>` to validate.
2. **Delete**: when a module is superseded or no longer needed, remove the
   archived file and add a one-line note here under the Removed section.
3. **CI enforcement**: `scripts/check-orphans.sh` ensures no NEW orphan
   files appear at `crates/<crate>/src/*.rs` without a `mod` declaration.
   Files inside `_archive/` are explicitly exempt.

## Inventory (87 files across 27 crates)

| Crate | File | Lines |
|-------|------|-------|
| `zbx-admin` | `access_control.rs` | 199 |
| `zbx-admin` | `audit_log.rs` | 192 |
| `zbx-admin` | `chain_params.rs` | 202 |
| `zbx-admin` | `emergency.rs` | 185 |
| `zbx-admin` | `governance.rs` | 469 |
| `zbx-admin` | `treasury.rs` | 167 |
| `zbx-admin` | `upgrades.rs` | 200 |
| `zbx-admin` | `validator_admin.rs` | 142 |
| `zbx-bridge` | `events.rs` | 64 |
| `zbx-bridge` | `lock_unlock.rs` | 301 |
| `zbx-bridge` | `validator_set.rs` | 137 |
| `zbx-consensus` | `bft.rs` | 82 |
| `zbx-consensus` | `block_producer.rs` | 178 |
| `zbx-consensus` | `bls_agg.rs` | 174 |
| `zbx-consensus` | `epoch.rs` | 262 |
| `zbx-consensus` | `finality.rs` | 34 |
| `zbx-consensus` | `fork_choice.rs` | 156 |
| `zbx-consensus` | `lmd_ghost.rs` | 234 |
| `zbx-consensus` | `validator_set.rs` | 49 |
| `zbx-contracts` | `erc1155.rs` | 144 |
| `zbx-contracts` | `erc4626.rs` | 125 |
| `zbx-contracts` | `proxy.rs` | 211 |
| `zbx-contracts` | `storage_layout.rs` | 202 |
| `zbx-evm` | `executor.rs` | 77 |
| `zbx-evm` | `state_backend.rs` | 47 |
| `zbx-explorer` | `contract_verify.rs` | 288 |
| `zbx-indexer` | `block_indexer.rs` | 239 |
| `zbx-indexer` | `event_indexer.rs` | 182 |
| `zbx-launchpad` | `ido.rs` | 169 |
| `zbx-lending` | `health_factor.rs` | 157 |
| `zbx-mempool` | `block_prune.rs` | 132 |
| `zbx-mempool` | `content_api.rs` | 148 |
| `zbx-mempool` | `eviction.rs` | 31 |
| `zbx-mempool` | `nonce_manager.rs` | 221 |
| `zbx-mempool` | `ordering.rs` | 93 |
| `zbx-mempool` | `pricing.rs` | 115 |
| `zbx-mempool` | `propagation.rs` | 224 |
| `zbx-mempool` | `remove.rs` | 285 |
| `zbx-mempool` | `validation.rs` | 122 |
| `zbx-metrics` | `counter.rs` | 21 |
| `zbx-metrics` | `gauge.rs` | 25 |
| `zbx-metrics` | `histogram.rs` | 86 |
| `zbx-metrics` | `registry.rs` | 16 |
| `zbx-network` | `bandwidth.rs` | 77 |
| `zbx-network` | `eth_protocol.rs` | 149 |
| `zbx-network` | `peer_manager.rs` | 276 |
| `zbx-oracle` | `chainlink.rs` | 219 |
| `zbx-oracle` | `twap.rs` | 187 |
| `zbx-oracle-optimistic` | `optimistic.rs` | 189 |
| `zbx-oracle-zk` | `zk_notary.rs` | 176 |
| `zbx-p2p` | `discovery.rs` | 27 |
| `zbx-p2p` | `gossip.rs` | 43 |
| `zbx-p2p` | `handler.rs` | 40 |
| `zbx-p2p` | `peer_manager.rs` | 68 |
| `zbx-p2p` | `sessions.rs` | 141 |
| `zbx-pool` | `block_prune.rs` | 126 |
| `zbx-pool` | `content_api.rs` | 139 |
| `zbx-pool` | `pending_pool.rs` | 171 |
| `zbx-pool` | `propagation.rs` | 271 |
| `zbx-pool` | `tx_validate.rs` | 175 |
| `zbx-rpc` | `fee_oracle.rs` | 229 |
| `zbx-rpc` | `subscription.rs` | 166 |
| `zbx-sequencer` | `base_fee.rs` | 315 |
| `zbx-sequencer` | `builder.rs` | 103 |
| `zbx-staking` | `delegation.rs` | 184 |
| `zbx-staking` | `epoch.rs` | 145 |
| `zbx-staking` | `lock.rs` | 189 |
| `zbx-staking` | `pool.rs` | 348 |
| `zbx-staking` | `reward_pool.rs` | 180 |
| `zbx-staking` | `tombstone.rs` | 172 |
| `zbx-state` | `account.rs` | 93 |
| `zbx-state` | `cache.rs` | 85 |
| `zbx-state` | `snapshot.rs` | 40 |
| `zbx-storage` | `iterator.rs` | 80 |
| `zbx-storage` | `kv_store.rs` | 257 |
| `zbx-storage` | `pruner.rs` | 35 |
| `zbx-storage` | `snapshot.rs` | 59 |
| `zbx-sync` | `bytecode_fetch.rs` | 141 |
| `zbx-sync` | `full_sync.rs` | 363 |
| `zbx-sync` | `range_proof.rs` | 128 |
| `zbx-sync` | `state_sync.rs` | 108 |
| `zbx-sync` | `warp_sync.rs` | 218 |
| `zbx-trie` | `encode.rs` | 145 |
| `zbx-tx` | `legacy.rs` | 148 |
| `zbx-types` | `native_addr.rs` | 94 |
| `zbx-types` | `zbx_nft.rs` | 50 |
| `zbx-zvm` | `bytecode.rs` | 129 |

## Removed

_(none yet — first deletions will be logged here with date + reason)_
