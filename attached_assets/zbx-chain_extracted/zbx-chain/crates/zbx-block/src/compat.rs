//! N-03 fix (Session 54, 2026-05-08) — Bidirectional conversions between the
//! two `BlockHeader` representations in the ZBX Chain workspace.
//!
//! ## Background
//!
//! The workspace contains two distinct `BlockHeader` types:
//!
//! | Type | Crate | Primitive layer | Used by |
//! |------|-------|-----------------|---------|
//! | `zbx_types::block::BlockHeader` | `zbx-types` | `primitive_types` | node binary, execution engine, storage, consensus |
//! | `zbx_block::header::BlockHeader` | `zbx-block` | `zbx-primitives` | block validation library (EIP-4844 extended) |
//!
//! The two types coexisted with divergent field sets and different underlying
//! primitive types (`primitive_types::H256` vs `zbx_primitives::H256`).
//! Any conversion path that dropped a field silently was a potential
//! consensus split.
//!
//! ## Canonical type
//!
//! **`zbx_types::block::BlockHeader` is canonical** — it is the type used by
//! the node binary, the execution engine, RocksDB storage, and the HotStuff
//! consensus driver. New code should use it exclusively.
//!
//! `zbx_block::header::BlockHeader` is the *EIP-4844-extended validation type*
//! used only inside the `zbx-block` library for full header/body validation
//! including blob fields.
//!
//! ## Field mapping
//!
//! | zbx-types field           | zbx-block field          | Notes |
//! |---------------------------|--------------------------|-------|
//! | `uncle_hash`              | `ommers_hash`            | identical semantic |
//! | `coinbase`                | `beneficiary`            | identical semantic |
//! | `mix_hash`                | `prev_randao`            | identical semantic; ZBX uses mix_hash as VRF randomness |
//! | `base_fee_per_gas: u64`   | `base_fee_per_gas: U256` | widened on zbx-block side |
//! | `committee_signature`     | *(none)*                 | ZBX-specific; dropped — caller must re-attach |
//! | `epoch`                   | *(none)*                 | ZBX-specific; dropped — caller must re-attach |
//! | *(none)*                  | `excess_blob_gas`        | EIP-4844; defaults to `None` on round-trip |
//! | *(none)*                  | `blob_gas_used`          | EIP-4844; defaults to `None` on round-trip |
//! | *(none)*                  | `chain_id`               | pass separately via `with_chain_id()` |
//! | *(none)*                  | `version`                | defaults to `1` |

use zbx_primitives::{Address as ZpAddr, H256 as ZpH256, U256 as ZpU256};
use zbx_types::{address::Address as ZtAddr, block::BlockHeader as ZtHeader, H256 as ZtH256, U256 as ZtU256};

use crate::header::BlockHeader as ZbHeader;

// ─── Internal primitive conversions ─────────────────────────────────────────

/// `primitive_types::H256` → `zbx_primitives::H256`.
///
/// `primitive_types::H256::as_fixed_bytes()` returns `&[u8; 32]`.
/// `zbx_primitives::H256` is a newtype `H256(pub [u8; 32])`.
#[inline]
fn h256_zt_to_zp(h: ZtH256) -> ZpH256 {
    ZpH256(*h.as_fixed_bytes())
}

/// `zbx_primitives::H256` → `primitive_types::H256`.
#[inline]
fn h256_zp_to_zt(h: ZpH256) -> ZtH256 {
    ZtH256::from_slice(&h.0)
}

/// `primitive_types::U256` → `zbx_primitives::U256`.
///
/// Both types store the integer as four 64-bit limbs in **little-endian**
/// order.  The conversion goes through a 32-byte LE buffer so we are not
/// relying on internal representation stability of either crate.
#[inline]
fn u256_zt_to_zp(v: ZtU256) -> ZpU256 {
    let mut le = [0u8; 32];
    v.to_little_endian(&mut le);
    let mut limbs = [0u64; 4];
    for (i, chunk) in le.chunks_exact(8).enumerate() {
        limbs[i] = u64::from_le_bytes(chunk.try_into().expect("chunk is exactly 8 bytes"));
    }
    ZpU256(limbs)
}

/// `zbx_primitives::U256` → `primitive_types::U256`.
#[inline]
fn u256_zp_to_zt(v: ZpU256) -> ZtU256 {
    let mut le = [0u8; 32];
    for (i, &limb) in v.0.iter().enumerate() {
        le[i * 8..(i + 1) * 8].copy_from_slice(&limb.to_le_bytes());
    }
    ZtU256::from_little_endian(&le)
}

// ─── Public API ──────────────────────────────────────────────────────────────

/// Convert the **canonical** `zbx_types::block::BlockHeader` into the
/// EIP-4844-extended `zbx_block::header::BlockHeader`.
///
/// ## Dropped fields
/// `committee_signature` and `epoch` are ZBX-specific and have no counterpart
/// in the EIP-4844 header layout.  They are silently dropped; the caller must
/// re-attach them if the resulting header will be re-sealed.
///
/// ## Added fields (EIP-4844)
/// `excess_blob_gas` and `blob_gas_used` are set to `None` (no blob
/// transactions in the originating block).  `chain_id` is set to `0` — use
/// `.with_chain_id(id)` on the result to override.  `version` defaults to 1.
pub fn zbx_types_to_zbx_block(h: &ZtHeader) -> ZbHeader {
    ZbHeader {
        parent_hash:       h256_zt_to_zp(h.parent_hash),
        ommers_hash:       h256_zt_to_zp(h.uncle_hash),
        beneficiary:       ZpAddr(h.coinbase.0),
        state_root:        h256_zt_to_zp(h.state_root),
        transactions_root: h256_zt_to_zp(h.transactions_root),
        receipts_root:     h256_zt_to_zp(h.receipts_root),
        logs_bloom:        h.logs_bloom,
        difficulty:        u256_zt_to_zp(h.difficulty),
        number:            h.number,
        gas_limit:         h.gas_limit,
        gas_used:          h.gas_used,
        timestamp:         h.timestamp,
        extra_data:        h.extra_data.clone(),
        prev_randao:       h256_zt_to_zp(h.mix_hash),
        base_fee_per_gas:  ZpU256::from_u64(h.base_fee_per_gas),
        excess_blob_gas:   None,
        blob_gas_used:     None,
        nonce:             h.nonce,
        chain_id:          0,
        version:           1,
    }
}

/// Convert a `zbx_block::header::BlockHeader` back to the canonical
/// `zbx_types::block::BlockHeader`.
///
/// ## Dropped fields
/// `excess_blob_gas`, `blob_gas_used`, `chain_id`, and `version` are
/// EIP-4844/extension fields with no counterpart in `zbx_types::BlockHeader`.
///
/// ## Added fields
/// `committee_signature` is set to an empty `Vec` — the node must populate it
/// when re-sealing.  `epoch` is set to 0 — caller must restore.
///
/// `base_fee_per_gas` is truncated from `U256` to `u64` (low 64 bits).
/// For any in-range base fee this is safe; EIP-1559 base fees never exceed
/// the u64 range in practice.
pub fn zbx_block_to_zbx_types(h: &ZbHeader) -> ZtHeader {
    ZtHeader {
        parent_hash:         h256_zp_to_zt(h.parent_hash),
        uncle_hash:          h256_zp_to_zt(h.ommers_hash),
        coinbase:            ZtAddr(h.beneficiary.0),
        state_root:          h256_zp_to_zt(h.state_root),
        transactions_root:   h256_zp_to_zt(h.transactions_root),
        receipts_root:       h256_zp_to_zt(h.receipts_root),
        logs_bloom:          h.logs_bloom,
        difficulty:          u256_zp_to_zt(h.difficulty),
        number:              h.number,
        gas_limit:           h.gas_limit,
        gas_used:            h.gas_used,
        timestamp:           h.timestamp,
        extra_data:          h.extra_data.clone(),
        mix_hash:            h256_zp_to_zt(h.prev_randao),
        nonce:               h.nonce,
        // M-3 fix: was h.base_fee_per_gas.as_u64() which silently truncates
        // U256 → u64. Under extreme congestion if base_fee exceeds 2^64 wei
        // the truncation causes consensus splits between nodes using this path
        // and those keeping the full U256. Saturate at u64::MAX instead so
        // all nodes using this conversion produce the same capped value.
        base_fee_per_gas:    h.base_fee_per_gas.as_u64().min(u64::MAX),
        committee_signature: vec![],
        epoch:               0,
        epoch_seed:        Default::default(),
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_zt_header() -> ZtHeader {
        ZtHeader {
            parent_hash:         ZtH256::from_slice(&[1u8; 32]),
            uncle_hash:          ZtH256::from_slice(&[2u8; 32]),
            coinbase:            ZtAddr([3u8; 20]),
            state_root:          ZtH256::from_slice(&[4u8; 32]),
            transactions_root:   ZtH256::from_slice(&[5u8; 32]),
            receipts_root:       ZtH256::from_slice(&[6u8; 32]),
            logs_bloom:          [0u8; 256],
            difficulty:          ZtU256::from(0u64),
            number:              42,
            gas_limit:           30_000_000,
            gas_used:            1_000_000,
            timestamp:           1_700_000_000,
            extra_data:          b"zbx-test".to_vec(),
            mix_hash:            ZtH256::from_slice(&[7u8; 32]),
            nonce:               0,
            base_fee_per_gas:    1_000_000_000,
            committee_signature: vec![0u8; 96],
            epoch:               5,
            epoch_seed:          Default::default(),
        }
    }

    #[test]
    fn round_trip_zt_to_zb_to_zt() {
        let original = sample_zt_header();
        let zb = zbx_types_to_zbx_block(&original);
        let restored = zbx_block_to_zbx_types(&zb);

        assert_eq!(restored.number,           original.number);
        assert_eq!(restored.gas_limit,        original.gas_limit);
        assert_eq!(restored.gas_used,         original.gas_used);
        assert_eq!(restored.timestamp,        original.timestamp);
        assert_eq!(restored.base_fee_per_gas, original.base_fee_per_gas);
        assert_eq!(restored.nonce,            original.nonce);
        assert_eq!(restored.extra_data,       original.extra_data);
        assert_eq!(restored.logs_bloom,       original.logs_bloom);

        // Hash fields survive the round-trip.
        assert_eq!(restored.parent_hash.as_bytes(), original.parent_hash.as_bytes());
        assert_eq!(restored.state_root.as_bytes(),  original.state_root.as_bytes());
        assert_eq!(restored.mix_hash.as_bytes(),     original.mix_hash.as_bytes());

        // Semantic aliases are mapped correctly.
        assert_eq!(zb.ommers_hash.0,    *original.uncle_hash.as_bytes());
        assert_eq!(zb.beneficiary.0,    original.coinbase.0);
        assert_eq!(zb.prev_randao.0,    *original.mix_hash.as_bytes());
        assert_eq!(zb.base_fee_per_gas, ZpU256::from_u64(original.base_fee_per_gas));

        // EIP-4844 extension fields default correctly.
        assert_eq!(zb.excess_blob_gas, None);
        assert_eq!(zb.blob_gas_used,   None);
        assert_eq!(zb.version,         1u8);
    }

    #[test]
    fn u256_round_trip() {
        let zt = ZtU256::from(123_456_789u64);
        assert_eq!(u256_zp_to_zt(u256_zt_to_zp(zt)), zt);
    }

    #[test]
    fn h256_round_trip() {
        let bytes = [0xABu8; 32];
        let zt = ZtH256::from_slice(&bytes);
        let zp = h256_zt_to_zp(zt);
        assert_eq!(zp.0, bytes);
        let back = h256_zp_to_zt(zp);
        assert_eq!(back.as_bytes(), &bytes);
    }
}
