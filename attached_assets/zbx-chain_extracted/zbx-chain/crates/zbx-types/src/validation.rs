//! Strict validation rules for transactions, blocks, and signatures.
//!
//! Type-and-codec layer. The enforcement-side (mempool admit, block-import
//! pre-check) lives in `zbx-tx`/`zbx-consensus` and consumes these helpers.
//!
//! Discipline (matches `execution.rs`, `governance.rs`):
//! - `validate()` runs in BOTH constructor AND `Decodable::decode`.
//! - `BTreeSet<SignatureScheme>` for `allowed_signature_schemes` so RLP is canonical.
//! - Newtype `Encodable` delegations use `inner.rlp_append(s)` (LESSON #11).
//! - `item_count() != N` field-count gate at top of every `decode`.

use std::collections::BTreeSet;

use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};
use serde::{Deserialize, Serialize};

use crate::execution::ExecutionLimits;

// ---------------------------------------------------------------------------
// SignatureScheme — enum of permitted cryptographic signature algorithms.
// ---------------------------------------------------------------------------

/// Discriminant for permitted signature schemes. Encoded as a single byte
/// in RLP. New schemes MUST be appended (never reorder existing values).
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum SignatureScheme {
    /// secp256k1 ECDSA — Ethereum-compatible default.
    Secp256k1 = 0,
    /// Ed25519 — fast, deterministic, used for validator votes.
    Ed25519 = 1,
    /// BLS12-381 — aggregatable; reserved for future consensus extensions.
    Bls12381 = 2,
}

impl SignatureScheme {
    pub fn to_u8(self) -> u8 {
        self as u8
    }
    pub fn from_u8(b: u8) -> Result<Self, DecoderError> {
        match b {
            0 => Ok(Self::Secp256k1),
            1 => Ok(Self::Ed25519),
            2 => Ok(Self::Bls12381),
            _ => Err(DecoderError::Custom("invalid signature scheme byte")),
        }
    }
}

impl Encodable for SignatureScheme {
    fn rlp_append(&self, s: &mut RlpStream) {
        // LESSON #11: direct delegation, not s.append(&u8).
        self.to_u8().rlp_append(s);
    }
}

impl Decodable for SignatureScheme {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        let b: u8 = rlp.as_val()?;
        Self::from_u8(b)
    }
}

// ---------------------------------------------------------------------------
// ValidationError — reject codes used across mempool/block-import.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ValidationError {
    /// Encoded transaction byte length exceeds `ExecutionLimits::max_tx_size`.
    TxTooLarge { actual: u32, max: u32 },
    /// Encoded block byte length exceeds `ExecutionLimits::max_block_size`.
    BlockTooLarge { actual: u32, max: u32 },
    /// `tx.gas_limit` is below the intrinsic-cost floor.
    GasTooLow { provided: u64, intrinsic_floor: u64 },
    /// `tx.gas_limit` exceeds `ExecutionLimits::max_tx_gas`.
    GasTooHigh { provided: u64, max: u64 },
    /// Caller used a signature scheme not in `allowed_signature_schemes`.
    DisallowedSignatureScheme(SignatureScheme),
    /// Signature blob length exceeds `max_signature_size`.
    SignatureTooLarge { actual: u16, max: u16 },
    /// Public key blob length exceeds `max_pubkey_size`.
    PubkeyTooLarge { actual: u16, max: u16 },
    /// `tx.chain_id` does not match the network.
    ChainIdMismatch { expected: u64, got: u64 },
    /// Calldata exceeds `max_call_data_size`.
    CallDataTooLarge { actual: u32, max: u32 },
    /// Validator set size exceeds `max_validator_set_size`.
    ValidatorSetTooLarge { actual: u32, max: u32 },
    /// Validator stake is below `min_validator_stake`.
    StakeBelowMinimum { provided: u128, minimum: u128 },
    /// Strict canonical encoding required and the input was non-canonical.
    NonCanonicalEncoding,
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TxTooLarge { actual, max } => {
                write!(f, "tx too large: {actual} > {max}")
            }
            Self::BlockTooLarge { actual, max } => {
                write!(f, "block too large: {actual} > {max}")
            }
            Self::GasTooLow {
                provided,
                intrinsic_floor,
            } => write!(
                f,
                "gas too low: provided {provided}, intrinsic floor {intrinsic_floor}"
            ),
            Self::GasTooHigh { provided, max } => {
                write!(f, "gas too high: provided {provided}, max {max}")
            }
            Self::DisallowedSignatureScheme(s) => {
                write!(f, "disallowed signature scheme: {s:?}")
            }
            Self::SignatureTooLarge { actual, max } => {
                write!(f, "signature too large: {actual} > {max}")
            }
            Self::PubkeyTooLarge { actual, max } => {
                write!(f, "pubkey too large: {actual} > {max}")
            }
            Self::ChainIdMismatch { expected, got } => {
                write!(f, "chain id mismatch: expected {expected}, got {got}")
            }
            Self::CallDataTooLarge { actual, max } => {
                write!(f, "calldata too large: {actual} > {max}")
            }
            Self::ValidatorSetTooLarge { actual, max } => {
                write!(f, "validator set too large: {actual} > {max}")
            }
            Self::StakeBelowMinimum { provided, minimum } => {
                write!(f, "stake below minimum: {provided} < {minimum}")
            }
            Self::NonCanonicalEncoding => write!(f, "non-canonical encoding"),
        }
    }
}

impl std::error::Error for ValidationError {}

// ---------------------------------------------------------------------------
// ValidationRules — chain-wide policy parameters.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ValidationRules {
    /// Maximum number of validators in the active set.
    pub max_validator_set_size: u32,
    /// Minimum stake (smallest unit) to be eligible for the validator set.
    pub min_validator_stake: u128,
    /// Maximum byte length of a single signature blob.
    pub max_signature_size: u16,
    /// Maximum byte length of a single public key blob.
    pub max_pubkey_size: u16,
    /// Maximum calldata size for a single transaction.
    pub max_call_data_size: u32,
    /// Permitted signature schemes (canonical-sorted on the wire).
    pub allowed_signature_schemes: BTreeSet<SignatureScheme>,
    /// If true, decoders MUST refuse non-canonical RLP forms (no leading
    /// zeros on integers, no oversized list-length encodings, etc.).
    pub require_strict_canonical_encoding: bool,
}

impl ValidationRules {
    /// Recommended mainnet defaults.
    pub fn mainnet_default() -> Self {
        let mut schemes = BTreeSet::new();
        schemes.insert(SignatureScheme::Secp256k1);
        schemes.insert(SignatureScheme::Ed25519);
        Self {
            max_validator_set_size: 128,
            min_validator_stake: 100 * 1_000_000_000_000_000_000u128, // 100 ZBX
            max_signature_size: 96, // BLS later, secp256k1 is 65, Ed25519 is 64
            max_pubkey_size: 65,    // secp256k1 uncompressed
            max_call_data_size: 64 * 1024,
            allowed_signature_schemes: schemes,
            require_strict_canonical_encoding: true,
        }
    }

    /// Recommended testnet/devnet defaults (looser).
    pub fn testnet_default() -> Self {
        let mut s = Self::mainnet_default();
        s.max_validator_set_size = 256;
        s.max_call_data_size = 128 * 1024;
        s.require_strict_canonical_encoding = false;
        s
    }

    pub fn validate(&self) -> Result<(), DecoderError> {
        if self.max_validator_set_size == 0 {
            return Err(DecoderError::Custom("max_validator_set_size must be > 0"));
        }
        if self.max_signature_size == 0 {
            return Err(DecoderError::Custom("max_signature_size must be > 0"));
        }
        if self.max_pubkey_size == 0 {
            return Err(DecoderError::Custom("max_pubkey_size must be > 0"));
        }
        if self.allowed_signature_schemes.is_empty() {
            return Err(DecoderError::Custom(
                "allowed_signature_schemes must be non-empty",
            ));
        }
        Ok(())
    }

    // --- Validators (caller-side) ---

    pub fn check_tx_size(
        &self,
        tx_bytes_len: u32,
        limits: &ExecutionLimits,
    ) -> Result<(), ValidationError> {
        if tx_bytes_len > limits.max_tx_size {
            return Err(ValidationError::TxTooLarge {
                actual: tx_bytes_len,
                max: limits.max_tx_size,
            });
        }
        Ok(())
    }

    pub fn check_block_size(
        &self,
        block_bytes_len: u32,
        limits: &ExecutionLimits,
    ) -> Result<(), ValidationError> {
        if block_bytes_len > limits.max_block_size {
            return Err(ValidationError::BlockTooLarge {
                actual: block_bytes_len,
                max: limits.max_block_size,
            });
        }
        Ok(())
    }

    pub fn check_tx_gas(
        &self,
        gas_limit: u64,
        intrinsic_floor: u64,
        limits: &ExecutionLimits,
    ) -> Result<(), ValidationError> {
        if gas_limit < intrinsic_floor {
            return Err(ValidationError::GasTooLow {
                provided: gas_limit,
                intrinsic_floor,
            });
        }
        if gas_limit > limits.max_tx_gas {
            return Err(ValidationError::GasTooHigh {
                provided: gas_limit,
                max: limits.max_tx_gas,
            });
        }
        Ok(())
    }

    pub fn check_signature_scheme(
        &self,
        scheme: SignatureScheme,
    ) -> Result<(), ValidationError> {
        if !self.allowed_signature_schemes.contains(&scheme) {
            return Err(ValidationError::DisallowedSignatureScheme(scheme));
        }
        Ok(())
    }

    pub fn check_signature_size(&self, sig_len: u16) -> Result<(), ValidationError> {
        if sig_len > self.max_signature_size {
            return Err(ValidationError::SignatureTooLarge {
                actual: sig_len,
                max: self.max_signature_size,
            });
        }
        Ok(())
    }

    pub fn check_pubkey_size(&self, pk_len: u16) -> Result<(), ValidationError> {
        if pk_len > self.max_pubkey_size {
            return Err(ValidationError::PubkeyTooLarge {
                actual: pk_len,
                max: self.max_pubkey_size,
            });
        }
        Ok(())
    }

    pub fn check_chain_id(&self, expected: u64, got: u64) -> Result<(), ValidationError> {
        if expected != got {
            return Err(ValidationError::ChainIdMismatch { expected, got });
        }
        Ok(())
    }

    pub fn check_call_data_size(&self, len: u32) -> Result<(), ValidationError> {
        if len > self.max_call_data_size {
            return Err(ValidationError::CallDataTooLarge {
                actual: len,
                max: self.max_call_data_size,
            });
        }
        Ok(())
    }

    pub fn check_validator_set_size(&self, n: u32) -> Result<(), ValidationError> {
        if n > self.max_validator_set_size {
            return Err(ValidationError::ValidatorSetTooLarge {
                actual: n,
                max: self.max_validator_set_size,
            });
        }
        Ok(())
    }

    pub fn check_validator_stake(&self, stake: u128) -> Result<(), ValidationError> {
        if stake < self.min_validator_stake {
            return Err(ValidationError::StakeBelowMinimum {
                provided: stake,
                minimum: self.min_validator_stake,
            });
        }
        Ok(())
    }
}

// u128 RLP helpers (rlp crate doesn't impl Encodable for u128).
fn append_u128(s: &mut RlpStream, v: u128) {
    s.append(&v.to_be_bytes().as_ref());
}
fn decode_u128(rlp: &Rlp) -> Result<u128, DecoderError> {
    let bytes: Vec<u8> = rlp.as_val()?;
    if bytes.len() != 16 {
        return Err(DecoderError::Custom("u128 must be 16 bytes BE"));
    }
    let mut buf = [0u8; 16];
    buf.copy_from_slice(&bytes);
    Ok(u128::from_be_bytes(buf))
}

impl Encodable for ValidationRules {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(7);
        s.append(&self.max_validator_set_size);
        append_u128(s, self.min_validator_stake);
        s.append(&self.max_signature_size);
        s.append(&self.max_pubkey_size);
        s.append(&self.max_call_data_size);
        // BTreeSet<SignatureScheme> as an inline RLP list of u8s. Sorted by
        // the Set itself; decode re-checks strict ascending order.
        s.begin_list(self.allowed_signature_schemes.len());
        for sch in &self.allowed_signature_schemes {
            s.append(sch);
        }
        s.append(&self.require_strict_canonical_encoding);
    }
}

impl Decodable for ValidationRules {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if rlp.item_count()? != 7 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        let max_validator_set_size: u32 = rlp.val_at(0)?;
        let min_validator_stake = decode_u128(&rlp.at(1)?)?;
        let max_signature_size: u16 = rlp.val_at(2)?;
        let max_pubkey_size: u16 = rlp.val_at(3)?;
        let max_call_data_size: u32 = rlp.val_at(4)?;

        let schemes_rlp = rlp.at(5)?;
        let mut schemes = BTreeSet::new();
        let mut prev: Option<u8> = None;
        for item in schemes_rlp.iter() {
            let s = SignatureScheme::decode(&item)?;
            let b = s.to_u8();
            if let Some(p) = prev {
                if b <= p {
                    return Err(DecoderError::Custom(
                        "allowed_signature_schemes must be strictly ascending",
                    ));
                }
            }
            prev = Some(b);
            schemes.insert(s);
        }

        let require_strict_canonical_encoding: bool = rlp.val_at(6)?;

        let s = Self {
            max_validator_set_size,
            min_validator_stake,
            max_signature_size,
            max_pubkey_size,
            max_call_data_size,
            allowed_signature_schemes: schemes,
            require_strict_canonical_encoding,
        };
        s.validate()?;
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rlp::{decode, encode};

    fn limits() -> ExecutionLimits {
        ExecutionLimits::MAINNET_DEFAULT
    }

    // --- SignatureScheme ---
    #[test]
    fn scheme_roundtrip() {
        for s in [
            SignatureScheme::Secp256k1,
            SignatureScheme::Ed25519,
            SignatureScheme::Bls12381,
        ] {
            let bytes = encode(&s);
            let back: SignatureScheme = decode(&bytes).unwrap();
            assert_eq!(s, back);
        }
    }

    #[test]
    fn scheme_decode_rejects_invalid_byte() {
        let bytes = encode(&7u8);
        let r: Result<SignatureScheme, _> = decode(&bytes);
        assert!(r.is_err());
    }

    // --- ValidationRules ---
    #[test]
    fn rules_mainnet_default_validates() {
        ValidationRules::mainnet_default().validate().unwrap();
    }

    #[test]
    fn rules_rejects_empty_schemes() {
        let mut r = ValidationRules::mainnet_default();
        r.allowed_signature_schemes.clear();
        assert!(r.validate().is_err());
    }

    #[test]
    fn rules_rejects_zero_max_validator_set() {
        let mut r = ValidationRules::mainnet_default();
        r.max_validator_set_size = 0;
        assert!(r.validate().is_err());
    }

    #[test]
    fn rules_rlp_round_trip() {
        let r = ValidationRules::mainnet_default();
        let bytes = encode(&r);
        let back: ValidationRules = decode(&bytes).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn rules_rlp_round_trip_testnet() {
        let r = ValidationRules::testnet_default();
        let bytes = encode(&r);
        let back: ValidationRules = decode(&bytes).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn rules_decode_rejects_wrong_field_count() {
        let mut s = RlpStream::new_list(6);
        s.append(&128u32);
        append_u128(&mut s, 0u128);
        s.append(&65u16);
        s.append(&65u16);
        s.append(&1024u32);
        s.begin_list(0);
        let bytes = s.out();
        let r: Result<ValidationRules, _> = decode(&bytes);
        assert!(matches!(r, Err(DecoderError::RlpIncorrectListLen)));
    }

    // --- check_tx_size / check_block_size ---
    #[test]
    fn check_tx_size_pass_and_fail() {
        let r = ValidationRules::mainnet_default();
        r.check_tx_size(1000, &limits()).unwrap();
        assert!(r
            .check_tx_size(limits().max_tx_size + 1, &limits())
            .is_err());
    }

    #[test]
    fn check_block_size_pass_and_fail() {
        let r = ValidationRules::mainnet_default();
        r.check_block_size(1000, &limits()).unwrap();
        assert!(r
            .check_block_size(limits().max_block_size + 1, &limits())
            .is_err());
    }

    // --- check_tx_gas ---
    #[test]
    fn check_tx_gas_below_floor_errors() {
        let r = ValidationRules::mainnet_default();
        let e = r.check_tx_gas(100, 21000, &limits()).unwrap_err();
        assert!(matches!(e, ValidationError::GasTooLow { .. }));
    }

    #[test]
    fn check_tx_gas_above_max_errors() {
        let r = ValidationRules::mainnet_default();
        let e = r
            .check_tx_gas(limits().max_tx_gas + 1, 21000, &limits())
            .unwrap_err();
        assert!(matches!(e, ValidationError::GasTooHigh { .. }));
    }

    #[test]
    fn check_tx_gas_in_band_passes() {
        let r = ValidationRules::mainnet_default();
        r.check_tx_gas(50_000, 21_000, &limits()).unwrap();
    }

    // --- check_signature_scheme / sig size / pk size ---
    #[test]
    fn check_signature_scheme_disallow_bls_by_default() {
        let r = ValidationRules::mainnet_default();
        let e = r
            .check_signature_scheme(SignatureScheme::Bls12381)
            .unwrap_err();
        assert!(matches!(
            e,
            ValidationError::DisallowedSignatureScheme(SignatureScheme::Bls12381)
        ));
    }

    #[test]
    fn check_signature_scheme_allow_secp256k1() {
        let r = ValidationRules::mainnet_default();
        r.check_signature_scheme(SignatureScheme::Secp256k1).unwrap();
    }

    #[test]
    fn check_signature_size_caps() {
        let r = ValidationRules::mainnet_default();
        r.check_signature_size(65).unwrap();
        assert!(r.check_signature_size(r.max_signature_size + 1).is_err());
    }

    #[test]
    fn check_pubkey_size_caps() {
        let r = ValidationRules::mainnet_default();
        r.check_pubkey_size(65).unwrap();
        assert!(r.check_pubkey_size(r.max_pubkey_size + 1).is_err());
    }

    // --- chain id / calldata / validator set / stake ---
    #[test]
    fn check_chain_id_rejects_mismatch() {
        let r = ValidationRules::mainnet_default();
        let e = r.check_chain_id(8989, 1).unwrap_err();
        assert!(matches!(e, ValidationError::ChainIdMismatch { .. }));
    }

    #[test]
    fn check_call_data_size_caps() {
        let r = ValidationRules::mainnet_default();
        r.check_call_data_size(1024).unwrap();
        assert!(r.check_call_data_size(r.max_call_data_size + 1).is_err());
    }

    #[test]
    fn check_validator_set_size_caps() {
        let r = ValidationRules::mainnet_default();
        r.check_validator_set_size(64).unwrap();
        assert!(r
            .check_validator_set_size(r.max_validator_set_size + 1)
            .is_err());
    }

    #[test]
    fn check_validator_stake_below_min_errors() {
        let r = ValidationRules::mainnet_default();
        let e = r.check_validator_stake(0).unwrap_err();
        assert!(matches!(e, ValidationError::StakeBelowMinimum { .. }));
    }
}
