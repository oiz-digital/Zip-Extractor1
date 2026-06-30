//! ABI encoding: static and dynamic types.

use crate::{error::AbiError, types::{AbiType, AbiValue}};

pub struct AbiEncoder;

impl AbiEncoder {
    /// Encode a sequence of (type, value) pairs as ABI calldata (no selector).
    pub fn encode(params: &[(AbiType, AbiValue)]) -> Result<Vec<u8>, AbiError> {
        let mut heads = Vec::new();
        let mut tails = Vec::new();

        let head_size: usize = params.iter().map(|(t, _)| t.head_size()).sum();
        let mut tail_offset = head_size;

        for (typ, val) in params {
            if typ.is_dynamic() {
                // Head is a uint256 offset.
                heads.extend_from_slice(&Self::encode_uint(tail_offset as u128));
                let encoded = Self::encode_value(typ, val)?;
                tail_offset += encoded.len();
                tails.push(encoded);
            } else {
                heads.extend_from_slice(&Self::encode_value(typ, val)?);
            }
        }

        let mut out = heads;
        for tail in tails {
            out.extend(tail);
        }
        Ok(out)
    }

    /// Encode a single value.
    pub fn encode_value(typ: &AbiType, val: &AbiValue) -> Result<Vec<u8>, AbiError> {
        match (typ, val) {
            (AbiType::Uint(_), AbiValue::Uint(n)) => Ok(Self::encode_uint(*n)),
            (AbiType::Int(_),  AbiValue::Int(n))  => Ok(Self::encode_int(*n)),
            (AbiType::Bool,    AbiValue::Bool(b))  => Ok(Self::encode_uint(if *b { 1 } else { 0 })),
            (AbiType::Address, AbiValue::Address(a)) => {
                let mut buf = [0u8; 32];
                buf[12..].copy_from_slice(a);
                Ok(buf.to_vec())
            }
            (AbiType::FixedBytes(n), AbiValue::FixedBytes(bs)) => {
                let mut buf = [0u8; 32];
                let copy_len = (*n as usize).min(bs.len());
                buf[..copy_len].copy_from_slice(&bs[..copy_len]);
                Ok(buf.to_vec())
            }
            (AbiType::Bytes, AbiValue::Bytes(bs)) => {
                Ok(Self::encode_dynamic_bytes(bs))
            }
            (AbiType::String, AbiValue::String(s)) => {
                Ok(Self::encode_dynamic_bytes(s.as_bytes()))
            }
            (AbiType::Array(inner_type), AbiValue::Array(items)) => {
                let mut out = Self::encode_uint(items.len() as u128);
                let inner_params: Vec<_> = items.iter().map(|v| (inner_type.as_ref().clone(), v.clone())).collect();
                let encoded = Self::encode(&inner_params.iter().map(|(t, v)| (t.clone(), v.clone())).collect::<Vec<_>>())?;
                out.extend(encoded);
                Ok(out)
            }
            (AbiType::Tuple(types), AbiValue::Tuple(vals)) => {
                let params: Vec<_> = types.iter().cloned().zip(vals.iter().cloned()).collect();
                Self::encode(&params)
            }
            _ => Err(AbiError::TypeMismatch {
                expected: format!("{:?}", typ),
                got: format!("{:?}", val),
            }),
        }
    }

    fn encode_uint(n: u128) -> Vec<u8> {
        let mut buf = [0u8; 32];
        buf[16..].copy_from_slice(&n.to_be_bytes());
        buf.to_vec()
    }

    fn encode_int(n: i128) -> Vec<u8> {
        let bytes = n.to_be_bytes();
        // Sign-extend to 32 bytes.
        let fill = if n < 0 { 0xff } else { 0x00 };
        let mut buf = [fill; 32];
        buf[16..].copy_from_slice(&bytes);
        buf.to_vec()
    }

    fn encode_dynamic_bytes(data: &[u8]) -> Vec<u8> {
        let mut out = Self::encode_uint(data.len() as u128);
        out.extend_from_slice(data);
        // Pad to 32-byte boundary.
        let rem = data.len() % 32;
        if rem != 0 {
            out.extend(std::iter::repeat(0u8).take(32 - rem));
        }
        out
    }
}