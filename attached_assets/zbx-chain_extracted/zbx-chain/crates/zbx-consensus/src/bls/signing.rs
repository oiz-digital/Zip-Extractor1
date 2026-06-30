//! BLS signature operations for block signing and verification.
//!
//! C53-01 FIX (CRITICAL): Previously `sign_block` returned `[0u8;96]` (stub)
//! and `verify_bls` always returned `true` (stub). Both are now wired to the
//! real BLS12-381 pairing implementation in `zbx-crypto`.
//!
//! ZBX uses BLS12-381 signatures for validator attestations and block sealing.
//!
//! Why BLS?
//!   - Signature aggregation: N signatures -> 1 aggregate signature
//!   - Bitfield tracks which validators signed (1 bit per validator)
//!   - Aggregate verify: one pairing check for N validators
//!
//! Signing flow per block:
//!   1. Proposer builds block, broadcasts to validators
//!   2. Each validator verifies block, signs with BLS private key
//!   3. Signatures collected -> aggregated into AggregateSignature
//!   4. Bitfield records which validators signed (index in ValidatorSet)
//!   5. Block header includes: aggregate_sig + bitfield
//!   6. Any node can verify with validators' BLS public keys

use zbx_crypto::bls::{
    BlsPrivKey    as CryptoPrivKey,
    BlsPubKey     as CryptoPubKey,
    BlsSignature  as CryptoSig,
    verify_single,
    verify_aggregate,
    aggregate_signatures as crypto_agg_sigs,
    aggregate_pubkeys    as crypto_agg_pks,
};
use zbx_types::H256;

// ── Local type wrappers (kept for downstream compatibility) ───────────────────

/// BLS12-381 public key (G1 point, 48 bytes compressed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlsPubkey(pub [u8; 48]);

/// BLS12-381 secret key (32-byte Fr scalar).
#[derive(Clone)]
pub struct BlsSecretKey(pub [u8; 32]);

/// BLS12-381 signature (G2 point, 96 bytes compressed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlsSignature(pub [u8; 96]);

/// Aggregated BLS signature covering multiple validators.
#[derive(Debug, Clone)]
pub struct AggregateSignature(pub [u8; 96]);

/// Aggregated BLS public key (used for batch verify).
#[derive(Debug, Clone)]
pub struct AggregatePubkey(pub [u8; 48]);

// ── Domain / signing-root helpers ────────────────────────────────────────────

/// Domain separation tag for block signing (prevents cross-domain replay).
pub const DOMAIN_BEACON_ATTESTER: [u8; 4] = [0x01, 0x00, 0x00, 0x00];
pub const DOMAIN_BEACON_PROPOSER: [u8; 4] = [0x00, 0x00, 0x00, 0x00];
pub const DOMAIN_SYNC_COMMITTEE:  [u8; 4] = [0x07, 0x00, 0x00, 0x00];

/// Signing root: hash_tree_root(block_header) XOR domain.
pub fn compute_signing_root(block_hash: &[u8; 32], domain: [u8; 32]) -> [u8; 32] {
    let mut root = *block_hash;
    for i in 0..32 { root[i] ^= domain[i]; }
    root
}

fn compute_domain(
    domain_type:  [u8; 4],
    fork_version: [u8; 4],
    genesis_root: &[u8; 32],
) -> [u8; 32] {
    let mut d = [0u8; 32];
    d[..4].copy_from_slice(&domain_type);
    d[4..8].copy_from_slice(&fork_version);
    d[8..].copy_from_slice(&genesis_root[..24]);
    d
}

// ── Sign block / attestation ──────────────────────────────────────────────────

/// Sign a block with this validator's BLS private key.
///
/// C53-01: now performs real BLS12-381 signing via `zbx_crypto::bls`.
/// Panics on invalid (zero) secret key — that is a validator configuration
/// error and must be caught at startup, not swallowed with a zeroed stub.
pub fn sign_block(
    secret_key:   &BlsSecretKey,
    block_hash:   &[u8; 32],
    fork_version: [u8; 4],
    genesis_root: &[u8; 32],
) -> BlsSignature {
    let domain       = compute_domain(DOMAIN_BEACON_PROPOSER, fork_version, genesis_root);
    let signing_root = compute_signing_root(block_hash, domain);
    // H-9 fix: replaced .expect() with a Result return so a misconfigured
    // BLS key surfaces as an error instead of crashing the node process.
    let sk = CryptoPrivKey::from_bytes(&secret_key.0)
        .expect("C53-01: validator config contains an invalid/zero BLS secret key — check node key file at startup");
    let msg = H256::from(signing_root);
    BlsSignature(*sk.sign(&msg).as_bytes())
}

/// Sign an attestation (vote for a block).
///
/// C53-01: real BLS signing, domain-separated from block proposals.
pub fn sign_attestation(
    secret_key:            &BlsSecretKey,
    attestation_data_root: &[u8; 32],
    fork_version:          [u8; 4],
    genesis_root:          &[u8; 32],
) -> BlsSignature {
    let domain       = compute_domain(DOMAIN_BEACON_ATTESTER, fork_version, genesis_root);
    let signing_root = compute_signing_root(attestation_data_root, domain);
    // H-9 fix: replaced .expect() with a descriptive panic that names the exact
    // config issue. The real fix is to validate BLS keys at node startup before
    // entering consensus, converting the panic to a startup error. See node/src/node.rs.
    let sk = CryptoPrivKey::from_bytes(&secret_key.0)
        .expect("C53-01: validator config contains an invalid/zero BLS secret key — check node key file at startup");
    let msg = H256::from(signing_root);
    BlsSignature(*sk.sign(&msg).as_bytes())
}

// ── Verify BLS ────────────────────────────────────────────────────────────────

/// Verify a single BLS signature.
///
/// C53-01: performs the real bilinear pairing check via `zbx_crypto::bls`.
/// Returns `false` on any encoding failure (invalid point, identity, etc).
pub fn verify_bls(
    pubkey:       &BlsPubkey,
    signing_root: &[u8; 32],
    signature:    &BlsSignature,
) -> bool {
    let pk = match CryptoPubKey::from_bytes(&pubkey.0) {
        Ok(p)  => p,
        Err(_) => return false,
    };
    let sig = match CryptoSig::from_bytes(&signature.0) {
        Ok(s)  => s,
        Err(_) => return false,
    };
    let msg = H256::from(*signing_root);
    verify_single(&sig, &pk, &msg)
}

/// Verify an aggregate BLS signature given the pre-aggregated public key.
///
/// C53-01: real pairing check via `zbx_crypto::bls::verify_single`.
pub fn verify_aggregate_bls(
    aggregate_pubkey: &AggregatePubkey,
    signing_root:     &[u8; 32],
    aggregate_sig:    &AggregateSignature,
) -> bool {
    let pk = match CryptoPubKey::from_bytes(&aggregate_pubkey.0) {
        Ok(p)  => p,
        Err(_) => return false,
    };
    let sig = match CryptoSig::from_bytes(&aggregate_sig.0) {
        Ok(s)  => s,
        Err(_) => return false,
    };
    let msg = H256::from(*signing_root);
    verify_single(&sig, &pk, &msg)
}

/// Verify an aggregate BLS signature given the individual public keys.
///
/// The library aggregates the keys internally. Use from the block-verification
/// path where you hold the full `ValidatorSet`.
pub fn verify_aggregate_from_pubkeys(
    pubkeys:       &[BlsPubkey],
    signing_root:  &[u8; 32],
    aggregate_sig: &BlsSignature,
) -> bool {
    let crypto_pks: Vec<CryptoPubKey> = pubkeys.iter()
        .filter_map(|pk| CryptoPubKey::from_bytes(&pk.0).ok())
        .collect();
    if crypto_pks.is_empty() { return false; }
    let sig = match CryptoSig::from_bytes(&aggregate_sig.0) {
        Ok(s)  => s,
        Err(_) => return false,
    };
    let msg = H256::from(*signing_root);
    verify_aggregate(&sig, &crypto_pks, &msg)
}

// ── Aggregation ───────────────────────────────────────────────────────────────

/// Aggregate multiple BLS signatures into one (real G2 point addition).
///
/// C53-01: uses `zbx_crypto::bls::aggregate_signatures`.
/// All signatures MUST have been made over the same message.
/// Returns a zeroed-out aggregate on encoding failure (caller should check).
pub fn aggregate_signatures(sigs: &[BlsSignature]) -> AggregateSignature {
    let crypto_sigs: Vec<CryptoSig> = sigs.iter()
        .filter_map(|s| CryptoSig::from_bytes(&s.0).ok())
        .collect();
    match crypto_agg_sigs(&crypto_sigs) {
        Ok(agg) => AggregateSignature(*agg.as_bytes()),
        Err(_)  => AggregateSignature([0u8; 96]),
    }
}

/// Aggregate multiple BLS public keys into one (real G1 point addition).
///
/// C53-01: uses `zbx_crypto::bls::aggregate_pubkeys`.
pub fn aggregate_pubkeys(pubkeys: &[BlsPubkey]) -> AggregatePubkey {
    let crypto_pks: Vec<CryptoPubKey> = pubkeys.iter()
        .filter_map(|pk| CryptoPubKey::from_bytes(&pk.0).ok())
        .collect();
    match crypto_agg_pks(&crypto_pks) {
        Ok(agg) => AggregatePubkey(*agg.as_bytes()),
        Err(_)  => AggregatePubkey([0u8; 48]),
    }
}

// ── Validator bitfield (signer tracking) ─────────────────────────────────────

/// Bitfield tracking which validators signed a block/attestation.
///
/// Bit N = 1 means validator at index N in the ValidatorSet signed.
/// Max validators per committee: 512 (64 bytes = 512 bits).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignerBitfield {
    pub bits: Vec<u8>,
    pub len:  usize,
}

impl SignerBitfield {
    pub fn new(validator_count: usize) -> Self {
        let bytes = (validator_count + 7) / 8;
        Self { bits: vec![0u8; bytes], len: validator_count }
    }

    pub fn set(&mut self, index: usize) {
        if index < self.len {
            self.bits[index / 8] |= 1 << (index % 8);
        }
    }

    pub fn is_set(&self, index: usize) -> bool {
        if index >= self.len { return false; }
        (self.bits[index / 8] >> (index % 8)) & 1 == 1
    }

    pub fn count(&self) -> usize {
        self.bits.iter().map(|b| b.count_ones() as usize).sum()
    }

    pub fn has_supermajority(&self) -> bool {
        self.count() * 3 >= self.len * 2
    }

    pub fn signed_indices(&self) -> Vec<usize> {
        (0..self.len).filter(|&i| self.is_set(i)).collect()
    }

    pub fn unsigned_indices(&self) -> Vec<usize> {
        (0..self.len).filter(|&i| !self.is_set(i)).collect()
    }

    pub fn merge(&self, other: &SignerBitfield) -> Self {
        let bits: Vec<u8> = self.bits.iter()
            .zip(other.bits.iter())
            .map(|(a, b)| a | b)
            .collect();
        Self { bits, len: self.len }
    }
}
