//! ZK-Oracle: SNARK-verified price proofs (ZEP-012).
//!
//! # Problem with normal oracles
//!
//! Standard oracle:
//!   Reporter says: "ETH = $3500" + signature
//!   Question: HOW did you get $3500? Did you make it up?
//!   Answer: We trust you because you staked 100 ZBX.
//!
//! ZK-Oracle:
//!   Reporter says: "ETH = $3500" + SNARK proof
//!   Question: HOW did you get $3500?
//!   Answer: I prove (without revealing source) that a VALID CEX signed this
//!           price response at timestamp T. You can verify on-chain.
//!
//! # Circuit
//!
//! The ZK circuit proves:
//!   1. I have a TLS-signed HTTP response from a known CEX (Binance/Coinbase/Kraken)
//!   2. That response contains price P for symbol S at timestamp T
//!   3. |P - claimed_price| < epsilon (small rounding error)
//!
//! Public inputs:  (symbol_hash, claimed_price, timestamp, verifier_key_hash)
//! Private inputs: (tls_response_bytes, tls_signature, cex_pubkey)
//!
//! # TLSNotary Integration
//!
//! Uses a TLSNotary-style approach:
//!   1. Reporter fetches price via TLS with a "Notary" co-signer
//!   2. Notary signs the TLS transcript (proves it's real, not fabricated)
//!   3. Reporter generates Groth16 proof from transcript + notary sig
//!   4. On-chain verifier checks proof in O(1)
//!
//! # Gas cost
//!
//! Groth16 verification on ZBX EVM: ~280,000 gas (BN254 precompile)
//! Worth it for high-value feeds (ZBX/USD: $10M+ TVL)

pub mod circuit;
pub mod proof;
pub mod verifier;
pub mod notary;
pub mod prover;

pub use proof::{ZkPriceProof, ZkPriceReport};
pub use verifier::ZkOracleVerifier;
pub use notary::NotaryAttestation;
pub use prover::{PriceProver, ProverError};
pub use circuit::{CircuitPublicInputs, CircuitPrivateInputs, PriceCircuit};