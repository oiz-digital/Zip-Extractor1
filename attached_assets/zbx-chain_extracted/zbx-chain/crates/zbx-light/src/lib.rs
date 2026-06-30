//! zbx-light — Light client for Zebvix Chain.
//!
//! A light client downloads and verifies only block headers + Merkle proofs,
//! without executing transactions or maintaining full state. It uses the
//! HotStuff QC (quorum certificate) from each header to verify finality.
//!
//! # Capabilities
//! - Header chain sync (from a trusted checkpoint or genesis)
//! - SPV (Simplified Payment Verification): prove tx inclusion
//! - Account state proofs via state trie Merkle path
//! - Storage proofs for contract storage slots
//! - Log (receipt) proofs for event verification
//!
//! # Trust model
//! Light clients trust:
//! 1. The QC embedded in headers (2f+1 BLS signatures from validators)
//! 2. The genesis block (hardcoded)
//! 3. At least one honest full node for data availability

pub mod header_chain;
pub mod ibc;
pub mod rpc;
pub mod spv;
pub mod sync;

pub use header_chain::{HeaderChain, LightHeader, Checkpoint};
pub use ibc::{
    IbcClientError, IbcClientRegistry, IbcHeight, IbcValidatorInfo,
    ZbxClientState, ZbxConsensusState, ZbxHeader, ZbxMisbehaviour, Fraction,
};
pub use spv::{SpvProof, AccountProof, StorageProof, TxProof};
pub use sync::LightSync;