//! zbx-codec — Multi-format binary serialisation.
//!
//! ZBX Chain communicates with multiple ecosystems:
//!
//! | Ecosystem  | Format | Why                                      |
//! |------------|--------|------------------------------------------|
//! | Ethereum   | RLP    | Transaction encoding (legacy)            |
//! | Ethereum 2 | SSZ    | Consensus layer, beacon chain proofs     |
//! | Solana     | Borsh  | Cross-chain bridge messages              |
//! | Polkadot   | SCALE  | XCM cross-chain messaging                |
//! | Internal   | Bincode| Fast node-to-node communication          |
//!
//! All formats share a common trait `ZbxEncode` / `ZbxDecode`.

pub mod borsh;
pub mod error;
pub mod scale;
pub mod ssz;

pub use error::CodecError;

/// Common encoding trait — all ZBX types implement this.
pub trait ZbxEncode {
    fn encode_rlp(&self)    -> Vec<u8> { vec![] }
    fn encode_ssz(&self)    -> Vec<u8> { vec![] }
    fn encode_borsh(&self)  -> Result<Vec<u8>, CodecError> { Ok(vec![]) }
    fn encode_scale(&self)  -> Vec<u8> { vec![] }
}

/// Common decoding trait.
pub trait ZbxDecode: Sized {
    fn decode_rlp(bytes: &[u8])   -> Result<Self, CodecError>;
    fn decode_ssz(bytes: &[u8])   -> Result<Self, CodecError>;
    fn decode_borsh(bytes: &[u8]) -> Result<Self, CodecError>;
    fn decode_scale(bytes: &[u8]) -> Result<Self, CodecError>;
}