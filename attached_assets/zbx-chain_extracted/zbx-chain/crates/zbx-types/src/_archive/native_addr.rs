//! Native ZBX sentinel address.
//!
//! In DeFi protocols on ZBX, smart contracts often need to refer to
//! the native coin (ZBX) in the same way they refer to ERC-20 tokens.
//! The convention is a sentinel address: 0xEeee...Eeee.
//!
//! This is the same convention used on Ethereum for ETH in:
//!   - Uniswap V3 (WETH pair detection)
//!   - Aave (native ETH deposit routing)
//!   - OpenSea (native ETH payment)
//!
//! # Usage
//! ```rust
//! if token_address == NATIVE_ZBX {
//!     // Transfer native ZBX, not an ERC-20
//!     transfer_native(recipient, amount)?;
//! } else {
//!     // Transfer ZRC20 token
//!     Zrc20::transfer(token_address, recipient, amount)?;
//! }
//! ```

use crate::Address;

/// Sentinel address representing native ZBX coin (not a ZRC20 token).
/// 0xEeeEeeeeEEeEeEeEeEeEeeEEEeEeeeEeEeEeEeEeEeEeEeEEeEeEe
pub const NATIVE_ZBX: Address = Address([
    0xEe, 0xeE, 0xee, 0xee, 0xEE, 0xeE, 0xeE, 0xeE, 0xeE, 0xEE,
    0xee, 0xee, 0xEE, 0xeE, 0xee, 0xee, 0xEe, 0xEe, 0xEE, 0xeE,
]);

/// Hex string form (for ABIs and logs).
pub const NATIVE_ZBX_HEX: &str = "0xEeeEeeeeEEeEeEeEeEeEeeEEEeEeeeEeEeEeEeEeEeEeEeEEeEeEe";

/// Returns true if address is the native ZBX sentinel.
pub fn is_native_zbx(addr: &Address) -> bool {
    addr == &NATIVE_ZBX
}

/// Returns the canonical token address for a coin.
/// If native: returns NATIVE_ZBX sentinel.
/// If ZRC20: returns the contract address.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum TokenAddress {
    Native,
    Zrc20([u8; 20]),
}

impl TokenAddress {
    pub fn from_bytes(addr: [u8; 20]) -> Self {
        if addr == NATIVE_ZBX.0 { Self::Native } else { Self::Zrc20(addr) }
    }

    pub fn to_bytes(&self) -> [u8; 20] {
        match self {
            Self::Native    => NATIVE_ZBX.0,
            Self::Zrc20(a) => *a,
        }
    }

    pub fn is_native(&self) -> bool { matches!(self, Self::Native) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_sentinel_correct_bytes() {
        let addr = NATIVE_ZBX.0;
        assert_eq!(addr[0], 0xEe);
        assert_eq!(addr[1], 0xeE);
        assert!(is_native_zbx(&NATIVE_ZBX));
    }

    #[test]
    fn non_native_not_sentinel() {
        let random_addr = Address([0x12; 20]);
        assert!(!is_native_zbx(&random_addr));
    }

    #[test]
    fn token_address_from_sentinel() {
        let ta = TokenAddress::from_bytes(NATIVE_ZBX.0);
        assert!(ta.is_native());
    }

    #[test]
    fn token_address_from_zrc20() {
        let contract = [0xAB; 20];
        let ta = TokenAddress::from_bytes(contract);
        assert!(!ta.is_native());
    }
}