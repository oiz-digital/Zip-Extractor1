//! Genesis allocation processing — converts GenesisSpec.alloc into state trie entries.

use crate::{GenesisError, spec::Allocation};
use std::collections::HashMap;

/// Parsed and validated genesis account.
#[derive(Debug, Clone)]
pub struct ParsedAccount {
    pub address:  [u8; 20],
    pub balance:  u128,
    pub nonce:    u64,
    pub code:     Vec<u8>,
    pub storage:  HashMap<[u8; 32], [u8; 32]>,
}

/// Parse and validate all genesis allocations.
pub fn parse_allocations(
    alloc: &HashMap<String, Allocation>,
) -> Result<Vec<ParsedAccount>, GenesisError> {
    let mut seen = std::collections::HashSet::new();
    let mut accounts = Vec::with_capacity(alloc.len());

    for (addr_str, alloc) in alloc {
        let addr_hex = addr_str.trim_start_matches("0x");
        if !seen.insert(addr_hex.to_ascii_lowercase()) {
            return Err(GenesisError::DuplicateAllocation(addr_str.clone()));
        }

        let mut addr = [0u8; 20];
        hex::decode_to_slice(addr_hex, &mut addr)
            .map_err(|_| GenesisError::Invalid(format!("bad address: {addr_str}")))?;

        let bal_hex = alloc.balance.trim_start_matches("0x");
        let balance = u128::from_str_radix(bal_hex, 16)
            .map_err(|_| GenesisError::AllocationOverflow {
                addr:    addr_str.clone(),
                balance: alloc.balance.clone(),
            })?;

        let code = match &alloc.code {
            Some(c) => hex::decode(c.trim_start_matches("0x"))
                .map_err(|_| GenesisError::Invalid(format!("bad code for {addr_str}")))?,
            None => vec![],
        };

        let mut storage = HashMap::new();
        if let Some(slots) = &alloc.storage {
            for (k, v) in slots {
                let mut key = [0u8; 32];
                let mut val = [0u8; 32];
                let kh = k.trim_start_matches("0x");
                let vh = v.trim_start_matches("0x");
                hex::decode_to_slice(format!("{kh:0>64}"), &mut key)
                    .map_err(|_| GenesisError::Invalid(format!("bad slot key {k}")))?;
                hex::decode_to_slice(format!("{vh:0>64}"), &mut val)
                    .map_err(|_| GenesisError::Invalid(format!("bad slot val {v}")))?;
                storage.insert(key, val);
            }
        }

        accounts.push(ParsedAccount { address: addr, balance, nonce: alloc.nonce, code, storage });
    }

    // Deterministic ordering: sort by address so state root is canonical.
    accounts.sort_by_key(|a| a.address);
    Ok(accounts)
}