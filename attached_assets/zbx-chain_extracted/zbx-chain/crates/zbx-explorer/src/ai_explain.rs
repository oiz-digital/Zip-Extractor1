//! AI Contract Explanation — plain-English explanation of deployed contracts.
//!
//! Used by the explorer at `/contract/{address}/explain`.

use serde::{Deserialize, Serialize};

/// A human-readable explanation of a smart contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractExplanation {
    pub address:        String,
    pub contract_type:  String,
    pub summary:        String,
    pub key_functions:  Vec<FunctionNote>,
    pub security_notes: Vec<String>,
    pub verified:       bool,
}

/// Human-readable note about a specific contract function.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionNote {
    pub selector:    String,  // 4-byte hex selector
    pub name:        String,
    pub description: String,
    pub is_payable:  bool,
    pub is_view:     bool,
}

/// Detect contract type from bytecode.
pub fn detect_contract_type(bytecode: &[u8]) -> &'static str {
    if bytecode.is_empty() {
        return "EOA (not a contract)";
    }

    let hex: String = bytecode.iter()
        .map(|b| format!("{:02x}", b))
        .collect();

    // ERC-20: transfer(address,uint256)
    if hex.contains("a9059cbb") { return "ERC-20 Token"; }
    // ERC-721: ownerOf(uint256)
    if hex.contains("6352211e") { return "ERC-721 NFT"; }
    // ERC-1155: balanceOfBatch
    if hex.contains("4e1273f4") { return "ERC-1155 Multi-Token"; }
    // Uniswap V2 AMM: swap
    if hex.contains("022c0d9f") { return "AMM Liquidity Pool"; }
    // Lending: borrow
    if hex.contains("c5ebeaec") { return "Lending Protocol"; }
    // Governor: propose
    if hex.contains("7d5e81e2") { return "Governance / DAO"; }
    // Bridge: depositFor
    if hex.contains("8340f549") { return "Bridge Contract"; }
    // ZRC20Creator: deployToken
    if hex.contains("c2e3a2d6") { return "Token Factory"; }

    "Custom Contract"
}

/// Generate a human-readable summary for a contract type.
pub fn summarize_contract(contract_type: &str) -> String {
    match contract_type {
        "ERC-20 Token" =>
            "A fungible token. Holders can transfer tokens to each other. \
             May support minting, burning, and pausing depending on configuration.".into(),
        "ERC-721 NFT" =>
            "An NFT (Non-Fungible Token) collection. Each token has a unique ID \
             and can be transferred or traded on NFT marketplaces.".into(),
        "ERC-1155 Multi-Token" =>
            "A multi-token contract supporting both fungible and non-fungible tokens. \
             Commonly used for game items and collectibles.".into(),
        "AMM Liquidity Pool" =>
            "An Automated Market Maker DEX pool. Users can swap between two tokens at \
             market rates. Liquidity providers deposit tokens to earn swap fees.".into(),
        "Lending Protocol" =>
            "A lending/borrowing protocol. Users deposit collateral to borrow other tokens. \
             Interest accrues over time; positions can be liquidated if undercollateralised.".into(),
        "Governance / DAO" =>
            "A governance contract. Token holders create and vote on proposals. \
             Approved proposals execute on-chain after a timelock delay.".into(),
        "Bridge Contract" =>
            "A cross-chain bridge. Locks tokens on this chain while signalling a mint \
             on the destination chain (or vice versa for withdrawals).".into(),
        "Token Factory" =>
            "A no-code token deployment factory. Users deploy new tokens without writing \
             Solidity by calling this factory with their desired parameters.".into(),
        "EOA (not a contract)" =>
            "This is a regular wallet address (Externally Owned Account), not a smart contract. \
             It can send transactions but contains no code.".into(),
        _ =>
            "A custom smart contract. Check the verified source code on the explorer \
             for the full function list and documentation.".into(),
    }
}

/// Check for common security risks in the bytecode hex.
pub fn detect_security_notes(bytecode_hex: &str) -> Vec<String> {
    let mut notes = Vec::new();

    // SELFDESTRUCT (0xff) — contract can be destroyed.
    if bytecode_hex.ends_with("ff") || bytecode_hex.contains("ff") {
        notes.push(
            "⚠️ SELFDESTRUCT opcode present — this contract can be permanently destroyed by its owner.".into()
        );
    }

    // Delegatecall (0xf4) without a proxy guard is a common vulnerability.
    if bytecode_hex.contains("f4") && !bytecode_hex.contains("608060405234") {
        // Heuristic: non-standard proxy with delegatecall.
    }

    // Missing ReentrancyGuard heuristic: no SSTORE before external call.
    // (Simplified — real detection requires CFG analysis.)

    notes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn erc20_detected() {
        // Bytecode containing the ERC-20 transfer selector.
        let bytecode = hex::decode("a9059cbb00000000").unwrap_or_default();
        assert_eq!(detect_contract_type(&bytecode), "ERC-20 Token");
    }

    #[test]
    fn empty_bytecode_is_eoa() {
        assert_eq!(detect_contract_type(&[]), "EOA (not a contract)");
    }

    #[test]
    fn unknown_bytecode_is_custom() {
        assert_eq!(detect_contract_type(&[0x60, 0x80, 0x60, 0x40, 0x52]), "Custom Contract");
    }

    #[test]
    fn summary_not_empty() {
        for ty in &[
            "ERC-20 Token", "ERC-721 NFT", "ERC-1155 Multi-Token",
            "AMM Liquidity Pool", "Custom Contract", "EOA (not a contract)",
        ] {
            let s = summarize_contract(ty);
            assert!(!s.is_empty(), "summary for {} must not be empty", ty);
        }
    }
}
