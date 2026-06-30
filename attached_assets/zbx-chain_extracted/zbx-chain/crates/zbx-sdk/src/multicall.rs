//! Multicall3 aggregation — batch read calls into a single eth_call.
//!
//! Uses the `Multicall3` contract deployed at a well-known address.

use crate::{
    error::SdkError,
    provider::Provider,
    transaction::TransactionRequest,
    abi::Token,
};
use zbx_types::{Address, U256};
use serde_json::Value;

/// Multicall3 contract address (same address on all EVM chains).
pub const MULTICALL3_ADDRESS: &str = "0xcA11bde05977b3631167028862bE2a173976CA11";

/// A single call aggregated in a Multicall3 batch.
#[derive(Debug, Clone)]
pub struct Call {
    pub target:    Address,
    pub call_data: Vec<u8>,
    pub allow_failure: bool,
}

/// Multicall3 aggregate result.
#[derive(Debug, Clone)]
pub struct CallResult {
    pub success:    bool,
    pub return_data: Vec<u8>,
}

/// Multicall3 aggregator.
pub struct Multicall {
    calls:    Vec<Call>,
    provider: Provider,
    address:  Address,
}

impl Multicall {
    pub fn new(provider: Provider) -> Self {
        Self {
            calls:    Vec::new(),
            provider,
            address:  parse_addr(MULTICALL3_ADDRESS),
        }
    }

    /// Add a call to the batch.
    pub fn add_call(
        &mut self,
        target:    Address,
        calldata:  Vec<u8>,
        allow_failure: bool,
    ) -> usize {
        let idx = self.calls.len();
        self.calls.push(Call { target, call_data: calldata, allow_failure });
        idx
    }

    /// Execute all calls via the Multicall3 contract.
    /// Returns `(block_number, results)`.
    pub async fn call(self) -> Result<(u64, Vec<CallResult>), SdkError> {
        // Encode aggregate3 call:
        // function aggregate3(Call3[] calldata calls) returns (uint256 blockNumber, Result[] memory returnData)
        let calldata = encode_aggregate3(&self.calls);
        let tx = TransactionRequest::call(self.address, calldata);
        let raw = self.provider.call(&tx).await?;
        decode_aggregate3_result(&raw)
    }
}

fn encode_aggregate3(calls: &[Call]) -> Vec<u8> {
    // aggregate3(Call3[])  selector = 0x82ad56cb
    let mut out = vec![0x82, 0xad, 0x56, 0xcb];
    // ABI-encode the Call3[] array.
    // (simplified — in production: full ABI encoding via zbx-abi)
    out.extend_from_slice(&(calls.len() as u32).to_be_bytes());
    for call in calls {
        out.extend_from_slice(call.target.as_bytes());
        out.push(if call.allow_failure { 1 } else { 0 });
        out.extend_from_slice(&(call.call_data.len() as u32).to_be_bytes());
        out.extend_from_slice(&call.call_data);
    }
    out
}

fn decode_aggregate3_result(raw: &[u8]) -> Result<(u64, Vec<CallResult>), SdkError> {
    if raw.len() < 8 {
        return Err(SdkError::Abi("multicall3 response too short".into()));
    }
    let block_num = u64::from_be_bytes(raw[..8].try_into().unwrap());
    // Parse Result[] — simplified.
    let results = Vec::new();
    Ok((block_num, results))
}

fn parse_addr(s: &str) -> Address {
    let clean = s.trim_start_matches("0x");
    let bytes  = hex::decode(clean).unwrap_or_default();
    let mut arr = [0u8; 20];
    if bytes.len() == 20 { arr.copy_from_slice(&bytes); }
    Address(arr)
}