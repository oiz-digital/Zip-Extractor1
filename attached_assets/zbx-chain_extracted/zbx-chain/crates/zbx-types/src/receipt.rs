//! Transaction receipt and event log types.

use crate::{address::Address, H256};
use serde_big_array::BigArray;
use serde::{Deserialize, Serialize};

/// An EVM event log emitted during transaction execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Log {
    /// Contract that emitted the log.
    pub address: Address,
    /// Indexed topics (up to 4; topic[0] is the event signature hash).
    pub topics: Vec<H256>,
    /// Non-indexed ABI-encoded data.
    pub data: Vec<u8>,
    /// Block number of the containing block.
    pub block_number: u64,
    /// Index of this log within the block.
    pub log_index: u32,
    /// Hash of the containing transaction.
    pub transaction_hash: H256,
    /// Index of the containing transaction within its block.
    pub transaction_index: u32,
}

/// Transaction execution status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum TxStatus {
    /// Execution reverted or ran out of gas.
    Failure = 0,
    /// Execution succeeded.
    Success = 1,
}

/// Full receipt produced after executing one transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionReceipt {
    /// EIP-658 status.
    pub status: TxStatus,
    /// Cumulative gas used up to and including this transaction in the block.
    pub cumulative_gas_used: u64,
    /// Bloom filter over logs (2048 bits).
    #[serde(with = "BigArray")]
    pub logs_bloom: [u8; 256],
    /// Event logs emitted by this transaction.
    pub logs: Vec<Log>,
    /// Hash of the transaction.
    pub transaction_hash: H256,
    /// Index within the block.
    pub transaction_index: u32,
    /// Block hash.
    pub block_hash: H256,
    /// Block number.
    pub block_number: u64,
    /// Sender address.
    pub from: Address,
    /// Recipient or None for contract creation.
    pub to: Option<Address>,
    /// Address of deployed contract, if any.
    pub contract_address: Option<Address>,
    /// Actual gas used by this transaction.
    pub gas_used: u64,
    /// Effective price paid per gas unit (wei).
    pub effective_gas_price: u64,
}

impl TransactionReceipt {
    pub fn is_success(&self) -> bool {
        self.status == TxStatus::Success
    }
}