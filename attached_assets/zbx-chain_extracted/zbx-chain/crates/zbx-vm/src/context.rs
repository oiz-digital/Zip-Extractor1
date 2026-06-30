//! EVM call context and transaction environment.

use zbx_types::{Address, U256, H256};
use serde::{Deserialize, Serialize};

/// An emitted EVM log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Log {
    pub address: Address,
    pub topics:  Vec<H256>,
    pub data:    Vec<u8>,
}

/// Transaction environment (block/tx parameters visible to the EVM).
#[derive(Debug, Clone)]
pub struct TxEnv {
    pub caller:    Address,
    pub gas_limit: u64,
    pub gas_price: U256,
    pub transact_to: TransactTo,
    pub value:     U256,
    pub data:      Vec<u8>,
    pub nonce:     u64,
    pub chain_id:  u64,
    /// EIP-2930 access list.
    pub access_list: Vec<(Address, Vec<U256>)>,
    /// EIP-1559 max fee.
    pub max_fee_per_gas: Option<U256>,
    /// EIP-1559 priority fee.
    pub max_priority_fee_per_gas: Option<U256>,
}

/// The destination of a transaction.
#[derive(Debug, Clone)]
pub enum TransactTo {
    /// Regular call to an existing account.
    Call(Address),
    /// Contract creation.
    Create,
}

/// Block environment visible to the EVM.
#[derive(Debug, Clone)]
pub struct BlockEnv {
    pub number:       u64,
    pub coinbase:     Address,
    pub timestamp:    u64,
    pub gas_limit:    u64,
    pub base_fee:     U256,
    pub difficulty:   U256,
    pub chain_id:     u64,
    pub prev_randao:  H256,
}

impl Default for BlockEnv {
    fn default() -> Self {
        Self {
            number:      0,
            coinbase:    Address::zero(),
            timestamp:   0,
            gas_limit:   30_000_000,
            base_fee:    U256::from(1_000_000_000u64),
            difficulty:  U256::zero(),
            chain_id:    zbx_types::CHAIN_ID_MAINNET,
            prev_randao: H256::zero(),
        }
    }
}

/// The call context for a single EVM frame.
#[derive(Debug, Clone)]
pub struct CallContext {
    pub caller:       Address,
    pub address:      Address,
    pub code_address: Address,
    pub apparent_value: U256,
    pub call_type:    CallType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallType {
    Call,
    DelegateCall,
    CallCode,
    StaticCall,
}

/// The full execution context passed to `Evm::transact`.
#[derive(Debug, Clone)]
pub struct Context {
    pub tx:    TxEnv,
    pub block: BlockEnv,
}