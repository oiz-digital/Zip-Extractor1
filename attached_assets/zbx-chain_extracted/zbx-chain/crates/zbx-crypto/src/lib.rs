//! zbx-crypto: Cryptographic primitives for the Zebvix blockchain.
//!
//! Modules:
//! - secp256k1  — transaction signing / address derivation (EVM-compatible)
//! - keccak     — hashing (keccak256, keccak512)
//! - bls        — BLS12-381 aggregate signatures for validator committees
//! - vrf        — Verifiable Random Function for block proposer selection
//! - merkle     — Binary Merkle tree with inclusion proofs
//! - mpt        — Ethereum-compatible Modified Patricia Merkle Trie

pub mod bls;
pub mod keccak;
pub mod oracle_state;
pub mod vault_state;
pub mod kzg;
pub mod merkle;
pub mod mpt;
pub mod secp256k1;
pub mod vrf;

#[cfg(any(test, feature = "testing"))]
pub mod test_keys;

pub use bls::{BlsPrivKey, BlsPubKey, BlsSignature};
pub use keccak::{keccak256, keccak512};
pub use merkle::{MerkleProof, MerkleTree};
pub use mpt::transactions_root_mpt;
pub use secp256k1::{
    PrivKey, PubKey, Signature,
    recover_signer, recover_personal_signer, recover_typed_data_signer,
    personal_sign, personal_sign_hash,
    sign_typed_data, eip712_hash,
    address_to_checksum, validate_checksum_address,
    normalize_v_eip155,
};