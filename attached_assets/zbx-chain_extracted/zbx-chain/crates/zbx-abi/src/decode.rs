//! ABI decoding.

use crate::{error::AbiError, types::{AbiType, AbiValue}};

pub struct AbiDecoder<'a> {
    data: &'a [u8],
}

impl<'a> AbiDecoder<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data }
    }

    /// Decode a sequence of ABI-encoded values.
    pub fn decode(&self, types: &[AbiType]) -> Result<Vec<AbiValue>, AbiError> {
        let mut results = Vec::with_capacity(types.len());
        let mut pos = 0usize;

        for typ in types {
            let (val, new_pos) = self.decode_one(typ, pos)?;
            results.push(val);
            pos = new_pos;
        }

        Ok(results)
    }

    fn read_word(&self, offset: usize) -> Result<[u8; 32], AbiError> {
        if offset + 32 > self.data.len() {
            return Err(AbiError::BufferTooShort {
                need: offset + 32,
                have: self.data.len(),
            });
        }
        let mut word = [0u8; 32];
        word.copy_from_slice(&self.data[offset..offset + 32]);
        Ok(word)
    }

    fn read_u256_as_usize(&self, offset: usize) -> Result<usize, AbiError> {
        let word = self.read_word(offset)?;
        // Only read the lower 8 bytes for practical sizes.
        let mut n = 0usize;
        for b in &word[24..] {
            n = n.checked_shl(8).ok_or(AbiError::Overflow)?
                .checked_add(*b as usize).ok_or(AbiError::Overflow)?;
        }
        Ok(n)
    }

    fn decode_one(&self, typ: &AbiType, pos: usize) -> Result<(AbiValue, usize), AbiError> {
        match typ {
            AbiType::Uint(_) => {
                let word = self.read_word(pos)?;
                let mut n = 0u128;
                for b in &word[16..] {
                    n = (n << 8) | (*b as u128);
                }
                Ok((AbiValue::Uint(n), pos + 32))
            }
            AbiType::Int(_) => {
                let word = self.read_word(pos)?;
                let negative = word[0] >= 0x80;
                let mut n = if negative { -1i128 } else { 0i128 };
                for b in &word[16..] {
                    n = (n << 8) | (*b as i128);
                }
                Ok((AbiValue::Int(n), pos + 32))
            }
            AbiType::Bool => {
                let word = self.read_word(pos)?;
                let b = match word[31] {
                    0 => false,
                    1 => true,
                    _ => return Err(AbiError::BadBool),
                };
                Ok((AbiValue::Bool(b), pos + 32))
            }
            AbiType::Address => {
                let word = self.read_word(pos)?;
                let mut addr = [0u8; 20];
                addr.copy_from_slice(&word[12..]);
                Ok((AbiValue::Address(addr), pos + 32))
            }
            AbiType::FixedBytes(n) => {
                let word = self.read_word(pos)?;
                Ok((AbiValue::FixedBytes(word[..*n as usize].to_vec()), pos + 32))
            }
            AbiType::Bytes => {
                let offset = self.read_u256_as_usize(pos)?;
                let len    = self.read_u256_as_usize(offset)?;
                let data   = self.data.get(offset + 32..offset + 32 + len)
                    .ok_or(AbiError::UnexpectedEnd)?;
                Ok((AbiValue::Bytes(data.to_vec()), pos + 32))
            }
            AbiType::String => {
                let offset = self.read_u256_as_usize(pos)?;
                let len    = self.read_u256_as_usize(offset)?;
                let data   = self.data.get(offset + 32..offset + 32 + len)
                    .ok_or(AbiError::UnexpectedEnd)?;
                let s = String::from_utf8(data.to_vec()).map_err(|_| AbiError::InvalidUtf8)?;
                Ok((AbiValue::String(s), pos + 32))
            }
            AbiType::Array(inner) => {
                let offset   = self.read_u256_as_usize(pos)?;
                let count    = self.read_u256_as_usize(offset)?;
                let items    = self.decode_sequence(inner, offset + 32, count)?;
                Ok((AbiValue::Array(items), pos + 32))
            }
            AbiType::Tuple(types) => {
                if typ.is_dynamic() {
                    let offset = self.read_u256_as_usize(pos)?;
                    let vals = self.decode_tuple(types, offset)?;
                    Ok((AbiValue::Tuple(vals), pos + 32))
                } else {
                    let vals = self.decode_tuple(types, pos)?;
                    let size = types.iter().map(|t| t.head_size()).sum::<usize>();
                    Ok((AbiValue::Tuple(vals), pos + size))
                }
            }
            AbiType::FixedArray(inner, count) => {
                let items = self.decode_sequence(inner, pos, *count)?;
                let size  = inner.head_size() * count;
                Ok((AbiValue::Array(items), pos + size))
            }
        }
    }

    fn decode_sequence(&self, typ: &AbiType, start: usize, count: usize) -> Result<Vec<AbiValue>, AbiError> {
        let mut items = Vec::with_capacity(count);
        let mut pos = start;
        for _ in 0..count {
            let (val, new_pos) = self.decode_one(typ, pos)?;
            items.push(val);
            pos = new_pos;
        }
        Ok(items)
    }

    fn decode_tuple(&self, types: &[AbiType], start: usize) -> Result<Vec<AbiValue>, AbiError> {
        let mut vals = Vec::with_capacity(types.len());
        let mut pos  = start;
        for typ in types {
            let (val, new_pos) = self.decode_one(typ, pos)?;
            vals.push(val);
            pos = new_pos;
        }
        Ok(vals)
    }
}