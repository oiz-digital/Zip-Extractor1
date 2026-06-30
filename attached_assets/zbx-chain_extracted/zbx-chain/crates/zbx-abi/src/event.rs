//! Event signature and log decoding.

use crate::{
    decode::AbiDecoder,
    error::AbiError,
    types::{AbiType, AbiValue},
};
use sha3::{Digest, Keccak256};
use zbx_types::H256;

/// A 32-byte Ethereum event topic (keccak256 of event signature).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EventSignature(pub H256);

impl EventSignature {
    /// Compute event topic from canonical signature like
    /// `Transfer(address,address,uint256)`.
    pub fn from_signature(sig: &str) -> Self {
        let hash = Keccak256::digest(sig.as_bytes());
        Self(H256::from_slice(&hash))
    }
}

/// An indexed or non-indexed event parameter.
#[derive(Debug, Clone)]
pub struct EventParam {
    pub name:    String,
    pub typ:     AbiType,
    pub indexed: bool,
}

/// An ABI event descriptor.
#[derive(Debug, Clone)]
pub struct AbiEvent {
    pub name:      String,
    pub params:    Vec<EventParam>,
    pub anonymous: bool,
    pub topic:     EventSignature,
}

impl AbiEvent {
    pub fn new(name: &str, params: Vec<EventParam>, anonymous: bool) -> Self {
        let sig = Self::build_signature(name, &params);
        let topic = if anonymous {
            EventSignature(H256::zero())
        } else {
            EventSignature::from_signature(&sig)
        };
        Self { name: name.to_string(), params, anonymous, topic }
    }

    fn build_signature(name: &str, params: &[EventParam]) -> String {
        let args: Vec<_> = params.iter().map(|p| p.typ.canonical()).collect();
        format!("{}({})", name, args.join(","))
    }

    /// Decode a log's topics + data into named ABI values.
    pub fn decode_log(
        &self,
        topics: &[H256],
        data: &[u8],
    ) -> Result<Vec<(String, AbiValue)>, AbiError> {
        let mut topic_iter = topics.iter();
        if !self.anonymous {
            // First topic is the event signature.
            let sig_topic = topic_iter.next().ok_or(AbiError::UnexpectedEnd)?;
            if *sig_topic != self.topic.0 {
                return Err(AbiError::TypeMismatch {
                    expected: format!("{:?}", self.topic.0),
                    got: format!("{:?}", sig_topic),
                });
            }
        }

        let indexed:     Vec<_> = self.params.iter().filter(|p| p.indexed).collect();
        let non_indexed: Vec<_> = self.params.iter().filter(|p| !p.indexed).collect();

        let mut result = Vec::with_capacity(self.params.len());

        // Decode indexed params from topics.
        for param in &indexed {
            let topic = topic_iter.next().ok_or(AbiError::UnexpectedEnd)?;
            let val = AbiDecoder::new(topic.as_bytes()).decode(&[param.typ.clone()])?
                .into_iter().next().ok_or(AbiError::UnexpectedEnd)?;
            result.push((param.name.clone(), val));
        }

        // Decode non-indexed params from data.
        let non_idx_types: Vec<_> = non_indexed.iter().map(|p| p.typ.clone()).collect();
        let decoded_data = AbiDecoder::new(data).decode(&non_idx_types)?;
        for (param, val) in non_indexed.iter().zip(decoded_data) {
            result.push((param.name.clone(), val));
        }

        // Re-order to match original param order.
        let mut ordered = vec![None; self.params.len()];
        for (name, val) in result {
            if let Some(idx) = self.params.iter().position(|p| p.name == name) {
                ordered[idx] = Some((name, val));
            }
        }
        Ok(ordered.into_iter().flatten().collect())
    }
}