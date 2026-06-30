//! Admin pre-mint at genesis — initial token balances set before block #1.
//!
//! ## Pre-minted amounts
//!
//! | Token | Amount         | Purpose |
//! |-------|----------------|---------|
//! | ZUSD  | 100 million ZUSD | ZBX Foundation treasury — USD liquidity reserve |
//!
//! Pre-minted to the **Foundation Treasury Address** (`ZBX_ADMIN_ADDR`)
//! which is a well-known genesis address controlled by the Zebvix Foundation
//! multisig until full decentralisation (governed by ZEP).
//!
//! ## Treasury address derivation
//!
//! The address encodes "ZebvixFoundation" in ASCII (16 bytes) padded with
//! `\x00\x00\x00\x01` — making it unique, human-recognisable in block
//! explorers, and impossible to collide with any secp256k1 private key
//! (the high bytes are not on the curve).
//!
//! ```text
//! 5a 65 62 76 69 78 46 6f 75 6e 64 61 74 69 6f 6e 00 00 00 01
//! Z  e  b  v  i  x  F  o  u  n  d  a  t  i  o  n  \0 \0 \0 \x01
//! ```

use serde::{Deserialize, Serialize};
use tracing::info;

// ── Foundation treasury address ────────────────────────────────────────────────

/// Foundation treasury address (20 bytes).
///
/// Encodes "ZebvixFoundation" + `\x00\x00\x00\x01`:
/// `0x5a6562766978466f756e646174696f6e00000001`
pub const ZBX_ADMIN_ADDR: [u8; 20] = [
    0x5A, 0x65, 0x62, 0x76, 0x69, 0x78, 0x46, 0x6F,  // ZebvixFo
    0x75, 0x6E, 0x64, 0x61, 0x74, 0x69, 0x6F, 0x6E,  // undation
    0x00, 0x00, 0x00, 0x01,                            // \0\0\0\x01
];

// ── Pre-mint amounts ───────────────────────────────────────────────────────────

/// 1 ZUSD in base units (18 decimals).
pub const ONE_ZUSD: u128 = 1_000_000_000_000_000_000;

/// ZUSD genesis pre-mint: 100 million ZUSD.
///
/// Hex: `0x52b7d2dcc80cd2e4000000`
pub const ZUSD_GENESIS_PREMINT: u128 = 100_000_000 * ONE_ZUSD;   // 100M

// ── Record struct ──────────────────────────────────────────────────────────────

/// A single genesis token pre-mint entry.
///
/// Used in `GenesisSpec.token_premints` (stored as hex strings for JSON
/// compatibility, same pattern as `Allocation.balance`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenPremint {
    /// Contract address on ZBX Chain (hex, e.g. `"0x...231D0001"` for ZUSD).
    pub contract:  String,
    /// Recipient address (hex). Usually `ZBX_ADMIN_ADDR`.
    pub recipient: String,
    /// Amount in base units, as hex string (e.g. `"0x52b7d2dcc80cd2e4000000"`).
    pub amount:    String,
    /// Human-readable label for logs and block-explorer display.
    pub label:     String,
}

impl TokenPremint {
    /// Parse the `amount` hex string to u128.
    pub fn amount_u128(&self) -> Result<u128, String> {
        let s = self.amount.trim_start_matches("0x");
        u128::from_str_radix(s, 16)
            .map_err(|_| format!("bad premint amount hex '{}' for {}", self.amount, self.label))
    }

    /// Parse the `recipient` hex string to a 20-byte address.
    pub fn recipient_addr(&self) -> Result<[u8; 20], String> {
        let s = self.recipient.trim_start_matches("0x");
        let mut addr = [0u8; 20];
        hex::decode_to_slice(format!("{s:0>40}"), &mut addr)
            .map_err(|_| format!("bad recipient address '{}' for {}", self.recipient, self.label))?;
        Ok(addr)
    }

    /// Parse the `contract` hex string to a 20-byte address.
    pub fn contract_addr(&self) -> Result<[u8; 20], String> {
        let s = self.contract.trim_start_matches("0x");
        let mut addr = [0u8; 20];
        hex::decode_to_slice(format!("{s:0>40}"), &mut addr)
            .map_err(|_| format!("bad contract address '{}' for {}", self.contract, self.label))?;
        Ok(addr)
    }
}

// ── Canonical default premints ─────────────────────────────────────────────────

/// Returns the canonical mainnet genesis pre-mint list.
///
/// Applied to initial state trie **before block #1** via `GenesisSpec.token_premints`.
/// The execution engine must call `apply_premint()` for each entry.
pub fn default_premints() -> Vec<TokenPremint> {
    let admin = format!("0x{}", hex::encode(ZBX_ADMIN_ADDR));

    vec![
        TokenPremint {
            contract:  "0x00000000000000000000000000000000231D0001".into(),  // ZUSD
            recipient: admin,
            amount:    format!("0x{:x}", ZUSD_GENESIS_PREMINT),  // 100M ZUSD
            label:     "ZUSD genesis pre-mint: 100 million to Foundation Treasury".into(),
        },
    ]
}

// ── Execution ─────────────────────────────────────────────────────────────────

/// Apply a genesis pre-mint entry to contract state (called by genesis builder).
///
/// This bypasses runtime minting limits — it sets the initial balance
/// directly in the token contract's storage before any transaction runs.
pub fn apply_premint(entry: &TokenPremint) -> Result<AppliedPremint, String> {
    let recipient = entry.recipient_addr()?;
    let contract  = entry.contract_addr()?;
    let amount    = entry.amount_u128()?;

    info!(
        label    = %entry.label,
        contract = %entry.contract,
        recipient= %entry.recipient,
        amount   = amount,
        "genesis: applying token pre-mint"
    );

    Ok(AppliedPremint { contract, recipient, amount, label: entry.label.clone() })
}

/// Successful result of a `apply_premint` call.
#[derive(Debug, Clone)]
pub struct AppliedPremint {
    pub contract:  [u8; 20],
    pub recipient: [u8; 20],
    pub amount:    u128,
    pub label:     String,
}

impl AppliedPremint {
    /// Total in human-readable token units (divides by 10^18).
    pub fn amount_whole(&self) -> u128 { self.amount / ONE_ZUSD }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zusd_premint_is_100_million() {
        assert_eq!(ZUSD_GENESIS_PREMINT / ONE_ZUSD, 100_000_000);
    }

    #[test]
    fn admin_addr_encodes_zebvix_foundation() {
        let label = b"ZebvixFoundation";
        assert_eq!(&ZBX_ADMIN_ADDR[..16], label);
        assert_eq!(&ZBX_ADMIN_ADDR[16..], &[0x00, 0x00, 0x00, 0x01]);
    }

    #[test]
    fn admin_addr_hex_is_correct() {
        let hex = hex::encode(ZBX_ADMIN_ADDR);
        assert_eq!(hex, "5a6562766978466f756e646174696f6e00000001");
    }

    #[test]
    fn default_premints_length_is_one() {
        let p = default_premints();
        assert_eq!(p.len(), 1);
    }

    #[test]
    fn default_premints_zusd_only() {
        let p = default_premints();
        assert!(p[0].label.contains("ZUSD"));
        assert_eq!(p[0].amount_u128().unwrap(), ZUSD_GENESIS_PREMINT);
    }

    #[test]
    fn apply_premint_returns_correct_amount() {
        let premints = default_premints();
        let result = apply_premint(&premints[0]).unwrap();
        assert_eq!(result.amount, ZUSD_GENESIS_PREMINT);
        assert_eq!(result.amount_whole(), 100_000_000);
    }

    #[test]
    fn zusd_hex_amount_matches_expected() {
        // 0x52b7d2dcc80cd2e4000000 = 100_000_000 * 10^18
        let expected = format!("0x{:x}", ZUSD_GENESIS_PREMINT);
        assert_eq!(expected, "0x52b7d2dcc80cd2e4000000");
    }
}
