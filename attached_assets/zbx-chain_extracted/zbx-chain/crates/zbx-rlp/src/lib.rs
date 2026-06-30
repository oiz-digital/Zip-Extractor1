//! zbx-rlp: RLP (Recursive Length Prefix) encoding and decoding.
//!
//! RLP is the primary serialisation format for Ethereum and Zebvix Chain wire
//! protocol, state encoding, and receipt encoding.
//!
//! Specification: <https://ethereum.org/en/developers/docs/data-structures-and-encoding/rlp/>

pub mod encode;
pub mod decode;
pub mod error;
pub mod stream;

pub use encode::{RlpStream, Encodable};
pub use decode::{Rlp, Decodable};
pub use error::RlpError;

/// Encode a single RLP-encodable item.
pub fn encode<T: Encodable>(value: &T) -> Vec<u8> {
    let mut stream = RlpStream::new();
    value.encode_into(&mut stream);
    stream.out()
}

/// Decode bytes into T.
pub fn decode<T: Decodable>(bytes: &[u8]) -> Result<T, RlpError> {
    let rlp = Rlp::new(bytes);
    T::decode_from(&rlp)
}