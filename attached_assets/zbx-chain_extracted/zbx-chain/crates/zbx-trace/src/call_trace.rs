//! Call-level trace (tree of CALL/DELEGATECALL/CREATE operations).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum CallType {
    Call,
    DelegateCall,
    StaticCall,
    CallCode,
    Create,
    Create2,
}

/// A single call frame in the call tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallTrace {
    #[serde(rename = "type")]
    pub call_type:  CallType,
    pub from:       String,          // 0x-prefixed
    pub to:         Option<String>,  // None for CREATE
    pub value:      Option<String>,  // in wei (hex)
    pub gas:        String,          // gas provided
    pub gas_used:   String,
    pub input:      String,          // calldata (hex)
    pub output:     Option<String>,  // return data or None if reverted
    pub error:      Option<String>,  // revert reason
    pub revert_reason: Option<String>,
    /// Nested calls (subcalls).
    pub calls:      Vec<CallTrace>,
}

impl CallTrace {
    pub fn new_call(from: [u8; 20], to: [u8; 20], value: u128, gas: u64, input: Vec<u8>) -> Self {
        Self {
            call_type:     CallType::Call,
            from:          format!("0x{}", hex::encode(from)),
            to:            Some(format!("0x{}", hex::encode(to))),
            value:         Some(format!("0x{:x}", value)),
            gas:           format!("0x{:x}", gas),
            gas_used:      String::from("0x0"),
            input:         format!("0x{}", hex::encode(&input)),
            output:        None,
            error:         None,
            revert_reason: None,
            calls:         vec![],
        }
    }

    pub fn add_subcall(&mut self, subcall: CallTrace) {
        self.calls.push(subcall);
    }

    /// Total gas used across this call and all subcalls.
    pub fn total_gas_used(&self) -> u64 {
        let own = u64::from_str_radix(self.gas_used.trim_start_matches("0x"), 16).unwrap_or(0);
        own + self.calls.iter().map(|c| c.total_gas_used()).sum::<u64>()
    }
}