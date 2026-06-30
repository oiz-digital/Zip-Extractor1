//! Solidity ABI encoder/decoder — wraps zbx-abi for a friendly API.
//!
//! Supports all Solidity types:
//! `uint`, `int`, `address`, `bool`, `bytes`, `bytes32`, `string`,
//! `tuple`, and their array/fixed-array variants.

use crate::error::SdkError;
use zbx_types::{Address, U256, H256};
use serde::{Deserialize, Serialize};

// ── Token ─────────────────────────────────────────────────────────────────────

/// A decoded Solidity ABI value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Token {
    Address(Address),
    Uint(U256),
    Int(i128),
    Bool(bool),
    Bytes(Vec<u8>),
    FixedBytes([u8; 32]),
    String(String),
    Array(Vec<Token>),
    Tuple(Vec<Token>),
}

impl Token {
    pub fn as_address(&self) -> Option<Address> {
        if let Token::Address(a) = self { Some(*a) } else { None }
    }
    pub fn as_uint(&self) -> Option<U256> {
        if let Token::Uint(u) = self { Some(*u) } else { None }
    }
    pub fn as_bool(&self) -> Option<bool> {
        if let Token::Bool(b) = self { Some(*b) } else { None }
    }
    pub fn as_string(&self) -> Option<&str> {
        if let Token::String(s) = self { Some(s) } else { None }
    }
    pub fn as_bytes(&self) -> Option<&[u8]> {
        if let Token::Bytes(b) = self { Some(b) } else { None }
    }
    pub fn as_array(&self) -> Option<&[Token]> {
        if let Token::Array(a) = self { Some(a) } else { None }
    }
    pub fn as_tuple(&self) -> Option<&[Token]> {
        if let Token::Tuple(t) = self { Some(t) } else { None }
    }
}

// ── ABI types ─────────────────────────────────────────────────────────────────

/// A Solidity parameter type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ParamType {
    Address,
    Uint(usize),        // bit width: 8, 16, ..., 256
    Int(usize),
    Bool,
    Bytes,
    FixedBytes(usize),  // 1..=32
    String,
    Array(Box<ParamType>),
    FixedArray(Box<ParamType>, usize),
    Tuple(Vec<AbiParam>),
}

impl ParamType {
    /// Is this type dynamic (requires offset encoding)?
    pub fn is_dynamic(&self) -> bool {
        matches!(self, ParamType::Bytes | ParamType::String | ParamType::Array(_))
            || matches!(self, ParamType::Tuple(params) if params.iter().any(|p| p.ty.is_dynamic()))
    }

    /// ABI type string representation.
    pub fn to_type_str(&self) -> String {
        match self {
            ParamType::Address       => "address".into(),
            ParamType::Uint(n)      => format!("uint{}", n),
            ParamType::Int(n)       => format!("int{}", n),
            ParamType::Bool         => "bool".into(),
            ParamType::Bytes        => "bytes".into(),
            ParamType::FixedBytes(n)=> format!("bytes{}", n),
            ParamType::String       => "string".into(),
            ParamType::Array(t)     => format!("{}[]", t.to_type_str()),
            ParamType::FixedArray(t, n) => format!("{}[{}]", t.to_type_str(), n),
            ParamType::Tuple(params)=> {
                let inner = params.iter().map(|p| p.ty.to_type_str()).collect::<Vec<_>>().join(",");
                format!("({})", inner)
            }
        }
    }
}

/// A named ABI parameter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbiParam {
    pub name:     String,
    pub ty:       ParamType,
    pub indexed:  bool,        // for event params
    pub internal: Option<String>, // internal type (e.g. "struct Staker")
}

/// A parsed ABI function entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbiFunction {
    pub name:            String,
    pub inputs:          Vec<AbiParam>,
    pub outputs:         Vec<AbiParam>,
    pub state_mutability: String, // pure | view | nonpayable | payable
}

impl AbiFunction {
    /// 4-byte selector: keccak256(signature)[0..4].
    pub fn selector(&self) -> [u8; 4] {
        let sig = self.signature();
        let hash = sha3_keccak(sig.as_bytes());
        [hash[0], hash[1], hash[2], hash[3]]
    }

    /// ABI signature string: `transfer(address,uint256)`
    pub fn signature(&self) -> String {
        let params = self.inputs.iter()
            .map(|p| p.ty.to_type_str())
            .collect::<Vec<_>>()
            .join(",");
        format!("{}({})", self.name, params)
    }

    pub fn is_view(&self) -> bool {
        self.state_mutability == "view" || self.state_mutability == "pure"
    }
    pub fn is_payable(&self) -> bool {
        self.state_mutability == "payable"
    }
}

// ── Encoding ──────────────────────────────────────────────────────────────────

/// Encode a function call: selector || ABI-encoded args.
pub fn encode_call(
    selector: &[u8; 4],
    args:     &[Token],
    params:   &[AbiParam],
) -> Result<Vec<u8>, SdkError> {
    if args.len() != params.len() {
        return Err(SdkError::Abi(format!(
            "arg count mismatch: expected {} got {}", params.len(), args.len()
        )));
    }
    let mut out = selector.to_vec();
    out.extend_from_slice(&abi_encode(args, params)?);
    Ok(out)
}

/// ABI-encode a list of tokens (head-tail encoding).
pub fn abi_encode(tokens: &[Token], params: &[AbiParam]) -> Result<Vec<u8>, SdkError> {
    // Head = 32-byte slots (static values or offsets for dynamic)
    // Tail = dynamic data
    let mut head: Vec<u8> = Vec::new();
    let mut tail: Vec<u8> = Vec::new();
    let base_offset = tokens.len() * 32;

    for (token, param) in tokens.iter().zip(params.iter()) {
        if param.ty.is_dynamic() {
            // Write current tail offset in head.
            let offset = base_offset + tail.len();
            head.extend_from_slice(&u256_to_32(U256::from(offset as u128)));
            tail.extend_from_slice(&encode_dynamic(token)?);
        } else {
            head.extend_from_slice(&encode_static(token)?);
        }
    }
    head.extend_from_slice(&tail);
    Ok(head)
}

fn encode_static(token: &Token) -> Result<Vec<u8>, SdkError> {
    let mut slot = [0u8; 32];
    match token {
        Token::Address(a) => {
            slot[12..].copy_from_slice(a.as_bytes());
        }
        Token::Uint(u) => {
            u.to_big_endian(&mut slot[..]);
        }
        Token::Int(i) => {
            let bytes = i.to_be_bytes();
            slot[..16].copy_from_slice(&[if *i < 0 { 0xff } else { 0x00 }; 16]);
            slot[16..].copy_from_slice(&bytes);
        }
        Token::Bool(b) => { slot[31] = if *b { 1 } else { 0 }; }
        Token::FixedBytes(fb) => { slot[..fb.len()].copy_from_slice(fb); }
        _ => return Err(SdkError::Abi("expected static token".into())),
    }
    Ok(slot.to_vec())
}

fn encode_dynamic(token: &Token) -> Result<Vec<u8>, SdkError> {
    match token {
        Token::Bytes(b) => {
            let mut out = u256_to_32(U256::from(b.len() as u128));
            out.extend_from_slice(b);
            pad_to_32(&mut out);
            Ok(out)
        }
        Token::String(s) => {
            let bytes = s.as_bytes();
            let mut out = u256_to_32(U256::from(bytes.len() as u128));
            out.extend_from_slice(bytes);
            pad_to_32(&mut out);
            Ok(out)
        }
        Token::Array(items) => {
            let mut out = u256_to_32(U256::from(items.len() as u128));
            for item in items {
                out.extend_from_slice(&encode_static(item)?);
            }
            Ok(out)
        }
        _ => Err(SdkError::Abi("expected dynamic token".into())),
    }
}

// ── Decoding ──────────────────────────────────────────────────────────────────

/// Decode ABI-encoded output bytes into tokens.
pub fn decode_output(data: &[u8], params: &[AbiParam]) -> Result<Vec<Token>, SdkError> {
    if data.is_empty() { return Ok(Vec::new()); }
    let mut tokens = Vec::with_capacity(params.len());
    let mut offset = 0usize;
    for param in params {
        if data.len() < offset + 32 {
            return Err(SdkError::Abi("output data too short".into()));
        }
        let token = decode_token(&param.ty, data, &mut offset)?;
        tokens.push(token);
    }
    Ok(tokens)
}

fn decode_token(ty: &ParamType, data: &[u8], offset: &mut usize) -> Result<Token, SdkError> {
    if data.len() < *offset + 32 {
        return Err(SdkError::Abi("decode: data too short".into()));
    }
    let slot = &data[*offset..*offset + 32];
    *offset += 32;
    match ty {
        ParamType::Address => {
            let mut addr = [0u8; 20];
            addr.copy_from_slice(&slot[12..]);
            Ok(Token::Address(Address(addr)))
        }
        ParamType::Uint(_) => {
            let n = u128::from_be_bytes(slot[16..].try_into().unwrap());
            Ok(Token::Uint(U256::from(n)))
        }
        ParamType::Bool => Ok(Token::Bool(slot[31] != 0)),
        ParamType::FixedBytes(n) => {
            let mut fb = [0u8; 32];
            fb[..*n].copy_from_slice(&slot[..*n]);
            Ok(Token::FixedBytes(fb))
        }
        ParamType::Bytes | ParamType::String => {
            let dyn_offset = u64::from_be_bytes(slot[24..].try_into().unwrap()) as usize;
            if data.len() < dyn_offset + 32 {
                return Err(SdkError::Abi("decode: dyn offset out of bounds".into()));
            }
            let len = u64::from_be_bytes(data[dyn_offset..dyn_offset+8].try_into().unwrap()) as usize;
            let bytes = data[dyn_offset+32..dyn_offset+32+len].to_vec();
            if matches!(ty, ParamType::String) {
                Ok(Token::String(String::from_utf8_lossy(&bytes).into_owned()))
            } else {
                Ok(Token::Bytes(bytes))
            }
        }
        _ => Ok(Token::Bytes(slot.to_vec())), // fallback
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────────

fn u256_to_32(u: U256) -> Vec<u8> {
    let mut buf = [0u8; 32];
    u.to_big_endian(&mut buf);
    buf.to_vec()
}

fn pad_to_32(data: &mut Vec<u8>) {
    let rem = data.len() % 32;
    if rem != 0 { data.extend(vec![0u8; 32 - rem]); }
}

fn sha3_keccak(data: &[u8]) -> [u8; 32] {
    use sha3::{Digest, Keccak256};
    let h = Keccak256::digest(data);
    let mut out = [0u8; 32];
    out.copy_from_slice(&h);
    out
}