//! zbx-abi: Solidity ABI (Application Binary Interface) codec.
//!
//! Implements ABI encoding/decoding as specified in the Solidity ABI spec
//! (https://docs.soliditylang.org/en/latest/abi-spec.html).
//!
//! # Capabilities
//!
//! - Static types: uint/int (8–256), bool, address, bytes1–bytes32
//! - Dynamic types: bytes, string, dynamic arrays, tuples
//! - Function selectors (keccak256 of signature, first 4 bytes)
//! - Event topics (keccak256 of event signature)
//! - Packed encoding (non-standard, used in some protocols)
//! - JSON ABI fragment parsing

pub mod error;
pub mod types;
pub mod encode;
pub mod decode;
pub mod function;
pub mod event;

pub use error::AbiError;
pub use types::{AbiType, AbiValue};
pub use encode::AbiEncoder;
pub use decode::AbiDecoder;
pub use function::{FunctionSelector, AbiFunction};
pub use event::{EventSignature, AbiEvent};