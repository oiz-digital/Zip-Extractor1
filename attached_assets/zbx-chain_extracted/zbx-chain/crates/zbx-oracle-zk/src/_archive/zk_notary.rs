//! ZK Notary -- TLS transcript proofs for oracle price authenticity.
//!
//! The ZK Notary solves the "trust in the oracle reporter" problem:
//!   Standard oracle: "We fetched BTC=$68,000 from Binance. Trust us."
//!   ZK Notary:       "Here is a SNARK proof that we received a signed TLS
//!                    response from Binance's server with BTC=$68,000.
//!                    You can verify this cryptographically on-chain."
//!
//! ## How it works
//!
//!   1. Oracle reporter makes an HTTPS request to Binance API
//!   2. TLS session is recorded with a Notary co-signer (ZBX notary node)
//!      The notary co-signs the session: "I witnessed this TLS exchange"
//!      (Notary can verify session integrity without seeing private data)
//!
//!   3. Reporter generates a SNARK proof over:
//!      - Private inputs: TLS session transcript, notary signature, server cert
//!      - Public inputs: symbol_hash, price, timestamp, notary_pubkey_hash
//!
//!   4. Proof is submitted on-chain with price data
//!   5. ZbxOracleVerifier contract verifies proof in ~280k gas (Groth16/BN254)
//!
//! ## Notary network
//!   ZBX runs 5 independent Notary nodes
//!   Each reporter must have >=3 notary co-signatures (threshold)
//!   Notary nodes are run by ZBX validators (different from oracle reporters)
//!
//! ## Privacy
//!   Notary sees: that a TLS session happened, its size and timing
//!   Notary does NOT see: request body, API keys, personal data
//!   This is the tlsnotary.org model adapted for ZBX oracle use

use serde_big_array::BigArray;
use serde::{Serialize, Deserialize};

// ── Notary node ───────────────────────────────────────────────────────────────

/// A ZBX Notary node that co-signs TLS sessions.
pub struct ZkNotary {
    pub id:         u32,
    pub address:    [u8; 20],
    pub bls_pubkey: [u8; 48],
    pub tls_pubkey: [u8; 32],  // X25519 key for TLS interception
}

/// Notary co-signature on a TLS session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsProof {
    /// Hash of the TLS session transcript (proves session authenticity)
    pub session_hash:    [u8; 32],
    /// The attested data extracted from the TLS response
    pub attested_data:   AttestedPrice,
    /// BLS signature from notary nodes (threshold aggregate)
    #[serde(with = "BigArray")]
    pub notary_sig:      [u8; 96],  // BLS aggregate signature
    /// Which notary nodes signed (bitfield)
    pub signers:         u8,
    /// Timestamp of the TLS session
    pub session_time:    u64,
}

/// Price data extracted from the TLS response (public output of ZK circuit).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttestedPrice {
    /// Symbol hash (keccak256("BTC/USD"))
    pub symbol_hash:     [u8; 32],
    /// Price value (8 decimal places)
    pub price:           i128,
    /// Timestamp from the exchange API response
    pub timestamp:       u64,
    /// Exchange identifier (keccak256("binance"))
    pub exchange_hash:   [u8; 32],
}

// ── ZK proof structures ───────────────────────────────────────────────────────

/// A ZK price proof (Groth16, BN254 curve).
/// Public inputs: [symbol_hash, price, timestamp, notary_pk_hash, exchange_hash]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZkPriceProof {
    /// Groth16 proof bytes (pi_a, pi_b, pi_c -- 256 bytes total)
    pub proof:          Vec<u8>,
    /// Public inputs to the circuit (serialized)
    pub public_inputs:  Vec<[u8; 32]>,
    /// The TLS proof that feeds into the ZK circuit
    pub tls_proof:      TlsProof,
    /// Verification key hash (identifies which circuit version)
    pub vk_hash:        [u8; 32],
}

impl ZkPriceProof {
    /// Verify the ZK proof on-chain (stub -- production uses bn254 pairing).
    /// Cost: ~280,000 gas (Groth16 verification on BN254).
    pub fn verify_on_chain(&self, expected_symbol: &[u8; 32]) -> ZkVerifyResult {
        // 1. Check symbol matches
        if self.tls_proof.attested_data.symbol_hash != *expected_symbol {
            return ZkVerifyResult::SymbolMismatch;
        }
        // 2. Check proof.public_inputs[0] == symbol_hash
        // 3. Verify Groth16 proof using BN254 pairing
        // 4. Verify notary BLS signatures (threshold >= 3 of 5)
        // stub:
        ZkVerifyResult::Valid {
            price:     self.tls_proof.attested_data.price,
            timestamp: self.tls_proof.attested_data.timestamp,
        }
    }
}

/// Result of on-chain ZK proof verification.
#[derive(Debug)]
pub enum ZkVerifyResult {
    /// Proof is valid -- price and timestamp are authenticated.
    Valid { price: i128, timestamp: u64 },
    /// Proof failed pairing check.
    InvalidProof,
    /// Symbol in proof does not match requested feed.
    SymbolMismatch,
    /// Notary threshold not met (< 3 of 5 signed).
    InsufficientNotaries,
    /// TLS session is too old (stale price).
    Stale,
}

// ── Notary request/response ───────────────────────────────────────────────────

/// Request sent to a notary node to co-sign a TLS session.
#[derive(Debug, Clone)]
pub struct NotaryRequest {
    pub reporter:      [u8; 20],
    pub session_hash:  [u8; 32],
    pub feed_id:       String,
    pub requested_at:  u64,
}

/// Notary node response.
#[derive(Debug, Clone)]
pub struct NotaryResponse {
    pub request_hash:  [u8; 32],
    pub notary_id:     u32,
    pub partial_sig:   [u8; 48],  // BLS partial signature
    pub signed_at:     u64,
}

/// Collect notary responses and aggregate when threshold is met.
pub struct TlsProofAggregator {
    pub threshold:  usize,  // 3 of 5
    pub responses:  Vec<NotaryResponse>,
}

impl TlsProofAggregator {
    pub fn new(threshold: usize) -> Self {
        Self { threshold, responses: Vec::new() }
    }

    pub fn add_response(&mut self, r: NotaryResponse) { self.responses.push(r); }

    pub fn has_threshold(&self) -> bool { self.responses.len() >= self.threshold }

    /// Aggregate partial BLS signatures into a single TlsProof.
    pub fn aggregate(&self, attested: AttestedPrice, session_time: u64) -> Option<TlsProof> {
        if !self.has_threshold() { return None; }
        let mut signers: u8 = 0;
        for (i, _r) in self.responses.iter().enumerate().take(5) {
            signers |= 1 << i;
        }
        // Production: BLS aggregate signature from partial sigs
        Some(TlsProof {
            session_hash:  [0u8; 32],
            attested_data: attested,
            notary_sig:    [0u8; 96],
            signers,
            session_time,
        })
    }
}