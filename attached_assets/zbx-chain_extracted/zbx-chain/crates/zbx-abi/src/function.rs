//! Function selector and ABI function descriptors.

use crate::{
    encode::AbiEncoder,
    decode::AbiDecoder,
    error::AbiError,
    types::{AbiType, AbiValue},
};
use sha3::{Digest, Keccak256};

/// A 4-byte Ethereum function selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FunctionSelector(pub [u8; 4]);

impl FunctionSelector {
    /// Compute selector from a canonical signature like `transfer(address,uint256)`.
    pub fn from_signature(sig: &str) -> Self {
        let hash = Keccak256::digest(sig.as_bytes());
        let mut bytes = [0u8; 4];
        bytes.copy_from_slice(&hash[..4]);
        Self(bytes)
    }
}

/// An ABI function description.
#[derive(Debug, Clone)]
pub struct AbiFunction {
    pub name: String,
    pub inputs:  Vec<(String, AbiType)>,
    pub outputs: Vec<(String, AbiType)>,
    pub selector: FunctionSelector,
}

impl AbiFunction {
    pub fn new(name: &str, inputs: Vec<(String, AbiType)>, outputs: Vec<(String, AbiType)>) -> Self {
        let sig = Self::build_signature(name, &inputs);
        let selector = FunctionSelector::from_signature(&sig);
        Self { name: name.to_string(), inputs, outputs, selector }
    }

    fn build_signature(name: &str, inputs: &[(String, AbiType)]) -> String {
        let args: Vec<_> = inputs.iter().map(|(_, t)| t.canonical()).collect();
        format!("{}({})", name, args.join(","))
    }

    /// Encode a call: 4-byte selector + ABI-encoded args.
    pub fn encode_call(&self, args: &[AbiValue]) -> Result<Vec<u8>, AbiError> {
        if args.len() != self.inputs.len() {
            return Err(AbiError::Encode(format!(
                "expected {} args, got {}", self.inputs.len(), args.len()
            )));
        }
        let params: Vec<_> = self.inputs.iter().zip(args.iter())
            .map(|((_, t), v)| (t.clone(), v.clone()))
            .collect();
        let mut out = self.selector.0.to_vec();
        out.extend(AbiEncoder::encode(&params)?);
        Ok(out)
    }

    /// Decode return data into output values.
    pub fn decode_return(&self, data: &[u8]) -> Result<Vec<AbiValue>, AbiError> {
        let out_types: Vec<_> = self.outputs.iter().map(|(_, t)| t.clone()).collect();
        AbiDecoder::new(data).decode(&out_types)
    }
}