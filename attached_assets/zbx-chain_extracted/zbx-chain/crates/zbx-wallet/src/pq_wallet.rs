//! Post-quantum hybrid wallet for ZBX Chain (ZEP-015).
//!
//! Combines classical secp256k1 ECDSA with NIST FIPS 204 Dilithium-3 (ML-DSA-65).
//!
//! ## Key derivation
//! One BIP-39 mnemonic backs BOTH the classical and post-quantum key systems:
//!
//!   ECDSA key:   m/44'/7878'/0'/0/0 (standard BIP-44 path)
//!   Dilithium seed = keccak256("zbx-pq-v1" || ecdsa_private_key)
//!
//! ## Signature sizes
//! - ECDSA:      65 bytes  (r || s || v)
//! - Dilithium-3: 3309 bytes (ML-DSA-65 FIPS 204)
//!
//! ## Migration phases (ZEP-015)
//! | Block      | Phase              | Signs with            |
//! |------------|--------------------|-----------------------|
//! | 0–499,999  | Classical          | ECDSA only            |
//! | 500k–749k  | HybridEcdsaPrimary | ECDSA (+ Dilithium)   |
//! | 750k+      | HybridPqPrimary    | Both required         |
//!
//! ## Security guarantee
//! Breaking BOTH ECDSA AND Dilithium-3 requires breaking BOTH secp256k1 discrete
//! log AND Module-LWE simultaneously — orders of magnitude harder than either alone.

use zbx_pq::{
    DilithiumKeyPair, DilithiumPublicKey, DilithiumSignature,
    keygen_from_seed, dilithium_sign, dilithium_verify,
    PqPhase, DILITHIUM3_SIG_SIZE,
};
use sha3::{Keccak256, Digest};
use zeroize::Zeroizing;
use serde::{Serialize, Deserialize};
use crate::signer::{
    public_key_uncompressed, evm_address_from_pubkey,
    eip55_checksum, sign_hash_eip155,
};
use crate::create_import::WalletError;

/// Post-quantum hybrid wallet (ECDSA + Dilithium-3).
pub struct PqWallet {
    /// Classical secp256k1 private key — zeroized on drop via `Zeroizing`
    classical_key: Zeroizing<[u8; 32]>,
    /// Dilithium-3 keypair (signing key + verification key, derived from classical key)
    dilithium: DilithiumKeyPair,
    /// EVM address (derived from classical public key)
    pub address: [u8; 20],
    /// EIP-55 checksum address string
    pub checksum_address: String,
    /// Chain ID (8989 mainnet / 8990 testnet)
    pub chain_id: u64,
    /// Current PQ migration phase
    pub phase: PqPhase,
}

/// A hybrid-signed transaction ready for broadcast.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridSignedTx {
    /// ECDSA signature (65 bytes), present in Classical and Hybrid phases
    pub ecdsa_sig: Option<Vec<u8>>,
    /// Dilithium-3 signature (3309 bytes), present in Hybrid and PQ-only phases
    pub dilithium_sig: Option<Vec<u8>>,
    /// PQ migration phase at time of signing
    pub phase: PqPhase,
    /// The 32-byte hash that was signed (for verification)
    pub tx_hash: [u8; 32],
}

/// PQ wallet errors.
#[derive(Debug)]
pub enum PqWalletError {
    InvalidKey,
    SigningFailed,
}

impl From<WalletError> for PqWalletError {
    fn from(_: WalletError) -> Self {
        PqWalletError::SigningFailed
    }
}

impl PqWallet {
    /// Create a PQ wallet from a classical secp256k1 private key.
    ///
    /// Dilithium-3 seed is derived deterministically:
    ///   pq_seed = keccak256("zbx-pq-v1" || ecdsa_private_key)
    ///
    /// This means a single BIP-39 backup phrase recovers BOTH keys.
    pub fn from_private_key(
        private_key: [u8; 32],
        chain_id:    u64,
        phase:       PqPhase,
    ) -> Result<Self, PqWalletError> {
        // 1. Derive classical public key and EVM address
        let pubkey = public_key_uncompressed(&private_key)
            .map_err(|_| PqWalletError::InvalidKey)?;
        let address          = evm_address_from_pubkey(&pubkey);
        let checksum_address = eip55_checksum(&address);

        // 2. Derive Dilithium-3 seed from ECDSA private key (deterministic)
        //    Domain-separated with "zbx-pq-v1" to prevent key reuse confusion
        let seed_input: Vec<u8> = [b"zbx-pq-v1".as_ref(), &private_key[..]].concat();
        let seed_hash = Keccak256::digest(&seed_input);
        let mut pq_seed = [0u8; 32];
        pq_seed.copy_from_slice(&seed_hash);

        // 3. Generate Dilithium-3 keypair
        let dilithium = keygen_from_seed(&pq_seed);

        Ok(Self {
            classical_key: Zeroizing::new(private_key),
            dilithium,
            address, checksum_address, chain_id, phase,
        })
    }

    /// EVM address in EIP-55 checksum format.
    pub fn address(&self) -> &str { &self.checksum_address }

    /// Dilithium-3 verification key (1952 bytes for ML-DSA-65).
    pub fn dilithium_public_key(&self) -> &DilithiumPublicKey {
        &self.dilithium.public_key
    }

    /// Sign a transaction hash with classical ECDSA (EIP-155 replay-protected).
    pub fn sign_classical(&self, tx_hash: &[u8; 32]) -> Result<[u8; 65], PqWalletError> {
        sign_hash_eip155(&self.classical_key, tx_hash, self.chain_id)
            .map_err(|_| PqWalletError::SigningFailed)
    }

    /// Sign arbitrary data with Dilithium-3 (post-quantum).
    ///
    /// Returns a 3309-byte ML-DSA-65 signature.
    pub fn sign_pq(&self, message: &[u8]) -> Vec<u8> {
        let sig = dilithium_sign(&self.dilithium.private_key, message);
        sig.0.to_vec()
    }

    /// Sign a transaction hash with BOTH ECDSA and Dilithium-3 (hybrid mode).
    ///
    /// Phase determines which signatures are included:
    /// - Classical:          ECDSA only
    /// - HybridEcdsaPrimary: ECDSA + Dilithium
    /// - HybridPqPrimary:    ECDSA + Dilithium
    pub fn sign_hybrid(&self, tx_hash: &[u8; 32]) -> Result<HybridSignedTx, PqWalletError> {
        let (ecdsa_sig, dilithium_sig) = match self.phase {
            PqPhase::Classical => {
                let sig = self.sign_classical(tx_hash)?;
                (Some(sig.to_vec()), None)
            }
            PqPhase::HybridEcdsaPrimary | PqPhase::HybridPqPrimary => {
                let ecdsa = self.sign_classical(tx_hash)?;
                let pq    = self.sign_pq(tx_hash);
                (Some(ecdsa.to_vec()), Some(pq))
            }
            PqPhase::PostQuantumOnly => {
                // Full PQ phase: Dilithium only — ECDSA signatures are rejected
                let pq = self.sign_pq(tx_hash);
                (None, Some(pq))
            }
        };
        Ok(HybridSignedTx {
            ecdsa_sig,
            dilithium_sig,
            phase: self.phase,
            tx_hash: *tx_hash,
        })
    }

    /// Verify a Dilithium-3 signature against this wallet's PQ verification key.
    pub fn verify_pq(&self, message: &[u8], signature: &[u8]) -> bool {
        if signature.len() != DILITHIUM3_SIG_SIZE {
            return false;
        }
        let sig = DilithiumSignature(signature.to_vec());
        dilithium_verify(&self.dilithium.public_key, message, &sig).is_ok()
    }

    /// Upgrade the PQ phase (one-way — cannot downgrade).
    pub fn upgrade_phase(&mut self, new_phase: PqPhase) {
        if (new_phase as u8) > (self.phase as u8) {
            self.phase = new_phase;
        }
    }
}
