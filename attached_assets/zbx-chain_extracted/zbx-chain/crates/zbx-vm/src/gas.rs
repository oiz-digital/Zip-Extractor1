//! EVM gas schedule: static costs, memory expansion, and EIP-2929 access lists.

use std::collections::HashSet;
use zbx_types::{Address, U256};

/// Warm/cold access costs (EIP-2929).
pub const GAS_COLD_ACCOUNT_ACCESS: u64 = 2_600;
pub const GAS_WARM_ACCESS:         u64 = 100;
pub const GAS_COLD_SLOAD:          u64 = 2_100;
pub const GAS_CALL_STIPEND:        u64 = 2_300;
pub const GAS_SELFDESTRUCT_NEW:    u64 = 25_000;
pub const GAS_LOG_DATA_BYTE:       u64 = 8;
pub const GAS_TX_DATA_NONZERO:     u64 = 16;
pub const GAS_TX_DATA_ZERO:        u64 = 4;
pub const GAS_TX_BASE:             u64 = 21_000;
pub const GAS_TX_CREATE:           u64 = 53_000;
pub const GAS_INITCODE_WORD:       u64 = 2;   // EIP-3860
pub const GAS_KECCAK_WORD:         u64 = 6;
pub const GAS_COPY_WORD:           u64 = 3;
pub const GAS_SSTORE_RESET:        u64 = 2_900;
pub const GAS_SSTORE_SET:          u64 = 20_000;
pub const GAS_SSTORE_CLEARS:       u64 = 4_800; // EIP-3529 refund
pub const GAS_EXP_BYTE:            u64 = 50;

/// Access list for EIP-2929 warm/cold tracking.
#[derive(Debug, Default, Clone)]
pub struct AccessList {
    warm_accounts: HashSet<Address>,
    warm_slots:    HashSet<(Address, U256)>,
}

impl AccessList {
    pub fn new() -> Self { Self::default() }

    /// Pre-warm addresses and slots (EIP-2930).
    pub fn pre_warm(&mut self, addrs: &[(Address, Vec<U256>)]) {
        for (addr, slots) in addrs {
            self.warm_accounts.insert(*addr);
            for slot in slots {
                self.warm_slots.insert((*addr, *slot));
            }
        }
    }

    /// Mark an address as accessed; returns the gas cost (cold or warm).
    pub fn access_address(&mut self, addr: Address) -> u64 {
        if self.warm_accounts.insert(addr) {
            GAS_COLD_ACCOUNT_ACCESS
        } else {
            GAS_WARM_ACCESS
        }
    }

    /// Mark a storage slot as accessed; returns gas cost.
    pub fn access_slot(&mut self, addr: Address, slot: U256) -> u64 {
        if self.warm_slots.insert((addr, slot)) {
            GAS_COLD_SLOAD
        } else {
            GAS_WARM_ACCESS
        }
    }

    pub fn is_warm_address(&self, addr: &Address) -> bool {
        self.warm_accounts.contains(addr)
    }

    pub fn is_warm_slot(&self, addr: &Address, slot: &U256) -> bool {
        self.warm_slots.contains(&(*addr, *slot))
    }
}

/// Gas refund counter (capped at 1/5 of gas used per EIP-3529).
#[derive(Debug, Default, Clone)]
pub struct GasRefund {
    accrued: u64,
}

impl GasRefund {
    pub fn add(&mut self, amount: u64) { self.accrued += amount; }
    pub fn sub(&mut self, amount: u64) { self.accrued = self.accrued.saturating_sub(amount); }

    /// Capped refund: min(accrued, gas_used / 5).
    pub fn capped(&self, gas_used: u64) -> u64 {
        self.accrued.min(gas_used / 5)
    }
}

/// Compute intrinsic gas for a transaction.
pub fn intrinsic_gas(
    data: &[u8],
    is_create: bool,
    access_list: &[(Address, Vec<U256>)],
) -> u64 {
    let mut gas = if is_create { GAS_TX_CREATE } else { GAS_TX_BASE };

    // Data gas.
    for &b in data {
        gas += if b == 0 { GAS_TX_DATA_ZERO } else { GAS_TX_DATA_NONZERO };
    }

    // Access list gas (EIP-2930): 2400 per address + 1900 per slot.
    for (_, slots) in access_list {
        gas += 2_400;
        gas += 1_900 * slots.len() as u64;
    }

    // Initcode gas (EIP-3860).
    if is_create {
        gas += GAS_INITCODE_WORD * ((data.len() as u64 + 31) / 32);
    }

    gas
}