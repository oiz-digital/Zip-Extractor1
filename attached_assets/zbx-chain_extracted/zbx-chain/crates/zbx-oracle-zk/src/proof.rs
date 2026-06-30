//! ZK price proof structures.

use serde_big_array::BigArray;
use serde::{Serialize, Deserialize};
use crate::notary::NotaryAttestation;

/// A Groth16 proof that a price came from a valid CEX response.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ZkPriceProof {
    /// Groth16 proof bytes (pi_a, pi_b, pi_c)
    #[serde(with = "BigArray")]
    pub groth16_a: [u8; 64],  // G1 point (x, y)
    #[serde(with = "BigArray")]
    pub groth16_b: [u8; 128], // G2 point (x0,x1, y0,y1)
    #[serde(with = "BigArray")]
    pub groth16_c: [u8; 64],  // G1 point (x, y)
    /// Public inputs to the circuit
    pub public_inputs: ZkPublicInputs,
}

/// Public inputs to the ZK price circuit (known to verifier).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ZkPublicInputs {
    /// SHA-256 of the feed symbol (e.g. H("ZBX/USD"))
    pub symbol_hash:      [u8; 32],
    /// The claimed price (8 decimals)
    pub price:            i128,
    /// Unix timestamp of the CEX response
    pub timestamp:        u64,
    /// SHA-256 of the verifying key (identifies which CEX)
    pub vk_hash:          [u8; 32],
    /// Notary's public key (identifies the TLS notary)
    #[serde(with = "BigArray")]
    pub notary_pubkey:    [u8; 33], // compressed secp256k1
}

/// A ZK-proven price report — combines proof + metadata.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ZkPriceReport {
    pub proof:        ZkPriceProof,
    pub notary_attest: NotaryAttestation,
    pub reporter:     [u8; 20],
}

impl ZkPriceProof {
    /// Concatenate the three flat point fields into the canonical
    /// uncompressed byte layout that arkworks `Proof<Bn254>` expects:
    /// `pi_a (G1, 64 B) || pi_b (G2, 128 B) || pi_c (G1, 64 B) = 256 B`.
    /// SEC-2026-05-09 Pass-17 — consumed by the real Groth16 verifier.
    pub fn proof_bytes_canonical(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64 + 128 + 64);
        out.extend_from_slice(&self.groth16_a);
        out.extend_from_slice(&self.groth16_b);
        out.extend_from_slice(&self.groth16_c);
        out
    }
}

impl ZkPriceReport {
    /// Verify this report's proof off-chain before submission.
    ///
    /// Checks performed:
    ///   1. Proof G1 points (pi_a, pi_c) are non-zero (all-zero = invalid curve point)
    ///   2. Public inputs are well-formed (price > 0, symbol_hash non-zero, ts > 0)
    ///   3. Reporter address is non-zero
    ///   4. Notary TLS attestation signature is valid (real secp256k1 verify)
    ///   5. Notary pubkey in proof matches the attestation pubkey (consistency)
    ///   6. Notary attested to an approved CEX (Binance, Coinbase, Kraken, OKX)
    ///
    /// Full Groth16 pairing check requires a verifying key from the ZEP-012
    /// trusted ceremony and will be wired once ZEP-012 is deployed.
    pub fn verify_locally(&self) -> bool {
        let pi = &self.proof.public_inputs;

        // 1. Proof G1 points must be non-zero (all-zero = point at infinity = invalid)
        if self.proof.groth16_a.iter().all(|&b| b == 0) {
            return false;
        }
        if self.proof.groth16_c.iter().all(|&b| b == 0) {
            return false;
        }

        // 2. Public input sanity
        if pi.price <= 0 {
            return false;
        }
        if pi.symbol_hash.iter().all(|&b| b == 0) {
            return false;
        }
        if pi.timestamp == 0 {
            return false;
        }
        if pi.vk_hash.iter().all(|&b| b == 0) {
            return false;
        }

        // 3. Reporter must be a real address (non-zero)
        if self.reporter.iter().all(|&b| b == 0) {
            return false;
        }

        // 4. Verify notary TLS attestation (real secp256k1 sig check)
        if !self.notary_attest.verify() {
            return false;
        }

        // 5. Notary pubkey in proof must match the attestation pubkey
        if pi.notary_pubkey != self.notary_attest.notary_pubkey {
            return false;
        }

        // 6. Attestation must be for an approved CEX
        self.notary_attest.is_approved_source()
    }
}