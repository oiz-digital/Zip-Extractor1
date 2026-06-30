//! EIP-712 typed structured data hashing and signing for ZBX Chain.
//!
//! EIP-712 prevents phishing attacks where users sign opaque hex blobs without
//! knowing what they're agreeing to. Instead, structured types are shown to the
//! user in a human-readable format by MetaMask / hardware wallets.
//!
//! ## Signing flow
//!   1. Define your message type: `Transfer(address to,uint256 amount)`
//!   2. Compute `domainSeparator = hashStruct(EIP712Domain{...})`
//!   3. Compute `structHash = hashStruct(Transfer{...})`
//!   4. Sign: `keccak256("\x19\x01" || domainSeparator || structHash)`
//!
//! ## ZBX domain (governance / registry / AMM)
//!   name:               "ZBX Chain"
//!   version:            "1"
//!   chainId:            8989 (mainnet) | 8990 (testnet)
//!   verifyingContract:  contract address

use sha3::{Keccak256, Digest};
use serde::{Serialize, Deserialize};

/// Solidity value encoding for EIP-712 struct fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SolidityValue {
    /// bool — encoded as 1 or 0, left-padded to 32 bytes
    Bool(bool),
    /// uint256 — raw 32-byte big-endian encoding
    Uint256([u8; 32]),
    /// int256 — raw 32-byte big-endian two's complement
    Int256([u8; 32]),
    /// address — 20 bytes, right-aligned in 32 bytes
    Address([u8; 20]),
    /// bytes32 — raw 32 bytes
    Bytes32([u8; 32]),
    /// string — encoded as keccak256(utf8_bytes)
    Str(String),
    /// bytes — encoded as keccak256(bytes)
    Bytes(Vec<u8>),
}

/// A field definition in an EIP-712 struct type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeField {
    /// Field name (e.g. "to", "amount", "deadline")
    pub name:      String,
    /// Solidity type string (e.g. "address", "uint256", "string")
    pub type_name: String,
}

/// Pre-computed EIP-712 payload ready for signing.
#[derive(Debug, Clone)]
pub struct TypedData {
    /// EIP-712 domain separator (32 bytes)
    pub domain_separator: [u8; 32],
    /// Struct hash of the message (32 bytes)
    pub struct_hash: [u8; 32],
}

impl TypedData {
    /// Compute the final EIP-712 signing hash.
    ///
    /// Result: keccak256("\x19\x01" || domain_separator || struct_hash)
    pub fn signing_hash(&self) -> [u8; 32] {
        let mut h = Keccak256::new();
        h.update(b"\x19\x01");
        h.update(&self.domain_separator);
        h.update(&self.struct_hash);
        h.finalize().into()
    }

    /// Build from pre-computed domain separator and struct hash.
    pub fn new(domain_separator: [u8; 32], struct_hash: [u8; 32]) -> Self {
        Self { domain_separator, struct_hash }
    }
}

/// Compute the EIP-712 domain separator for ZBX Chain contracts.
///
/// Covers the standard EIP712Domain struct:
/// `EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)`
///
/// # Parameters
/// - `name`                – dApp / protocol name (e.g. "ZBX AMM")
/// - `version`             – version string (e.g. "1")
/// - `chain_id`            – 8989 mainnet / 8990 testnet
/// - `verifying_contract`  – address of the contract being interacted with
pub fn zbx_domain_separator(
    name:               &str,
    version:            &str,
    chain_id:           u64,
    verifying_contract: &[u8; 20],
) -> [u8; 32] {
    // EIP-712 type hash for the domain struct
    let type_hash = keccak256(
        b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"
    );

    // ABI-encode: each field packed as 32 bytes
    let mut enc = Vec::with_capacity(5 * 32);
    enc.extend_from_slice(&type_hash);
    enc.extend_from_slice(&keccak256(name.as_bytes()));
    enc.extend_from_slice(&keccak256(version.as_bytes()));

    // uint256 chainId — big-endian, left-padded to 32 bytes
    let mut chain_enc = [0u8; 32];
    chain_enc[24..].copy_from_slice(&chain_id.to_be_bytes());
    enc.extend_from_slice(&chain_enc);

    // address — right-aligned in 32 bytes
    let mut addr_enc = [0u8; 32];
    addr_enc[12..].copy_from_slice(verifying_contract);
    enc.extend_from_slice(&addr_enc);

    keccak256(&enc)
}

/// Encode a single EIP-712 value as 32 bytes.
///
/// Follows EIP-712 encoding rules:
/// - Primitive types (bool, uint, int, address, bytes32) → 32-byte ABI encoding
/// - Dynamic types (string, bytes) → keccak256 of the content
pub fn encode_value(value: &SolidityValue) -> [u8; 32] {
    match value {
        SolidityValue::Bool(b) => {
            let mut out = [0u8; 32];
            out[31] = *b as u8;
            out
        }
        SolidityValue::Uint256(v)
        | SolidityValue::Int256(v)
        | SolidityValue::Bytes32(v) => *v,
        SolidityValue::Address(a) => {
            let mut out = [0u8; 32];
            out[12..].copy_from_slice(a);
            out
        }
        SolidityValue::Str(s)   => keccak256(s.as_bytes()),
        SolidityValue::Bytes(b) => keccak256(b),
    }
}

/// Compute the EIP-712 `hashStruct` for a named type and field values.
///
/// hashStruct(S) = keccak256(typeHash(S) || enc(field_1) || enc(field_2) || ...)
///
/// # Parameters
/// - `type_string` – full Solidity type string, e.g. `"Transfer(address to,uint256 amount)"`
/// - `fields`      – ordered list of `(name, value)` pairs (name currently unused, kept for clarity)
pub fn hash_struct(type_string: &str, fields: &[(&str, SolidityValue)]) -> [u8; 32] {
    let type_hash = keccak256(type_string.as_bytes());
    let mut enc = Vec::with_capacity((1 + fields.len()) * 32);
    enc.extend_from_slice(&type_hash);
    for (_, value) in fields {
        enc.extend_from_slice(&encode_value(value));
    }
    keccak256(&enc)
}

/// Convenience: Build a `TypedData` from a domain + one message struct.
pub fn build_typed_data(
    domain_separator: [u8; 32],
    type_string:      &str,
    fields:           &[(&str, SolidityValue)],
) -> TypedData {
    TypedData {
        domain_separator,
        struct_hash: hash_struct(type_string, fields),
    }
}

/// keccak256 convenience wrapper.
pub fn keccak256(data: &[u8]) -> [u8; 32] {
    Keccak256::digest(data).into()
}
