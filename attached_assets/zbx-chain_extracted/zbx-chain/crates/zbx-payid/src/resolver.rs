//! Pay ID resolver: ali@zbx → on-chain address lookup.

use crate::{parser::{parse_pay_id, validate_display_name, PayIdParts}, error::PayIdError, format_pay_id};
use tracing::{debug, info};

/// Result of resolving a Pay ID.
#[derive(Debug, Clone)]
pub struct ResolvedPayId {
    /// Original input (as typed by user).
    pub input: String,
    /// Canonical Pay ID: "salman@zbx".
    pub pay_id: String,
    /// Registered full display name: e.g. "Salman Tyagi".
    /// Always present — registration enforces this as mandatory.
    pub display_name: String,
    /// Resolved ZBX wallet address.
    pub address: String,
    /// Whether the wallet is an AA smart wallet.
    pub is_smart_wallet: bool,
    /// Chain ID (8989 for ZBX mainnet, 8990 for testnet+devnet).
    pub chain_id: u64,
}

/// Request to register a new Pay ID on-chain.
/// All fields are mandatory — registration will fail without them.
#[derive(Debug, Clone)]
pub struct RegistrationRequest {
    /// Desired Pay ID handle — only the name part (e.g. "salman", not "salman@zbx").
    /// Rules: 3–32 chars, lowercase alphanumeric + hyphen, no leading/trailing hyphen.
    pub pay_id_name: String,
    /// Full legal/display name of the owner — MANDATORY.
    /// Rules: 2–64 chars, letters and spaces only (e.g. "Salman Tyagi").
    pub display_name: String,
    /// Owner's ZBX wallet address (20 bytes hex).
    pub address: String,
}

pub struct PayIdResolver {
    rpc_url: String,
    contract: String,
}

impl PayIdResolver {
    pub fn new(rpc_url: impl Into<String>) -> Self {
        PayIdResolver {
            rpc_url: rpc_url.into(),
            contract: crate::PAYID_CONTRACT.to_string(),
        }
    }

    /// Register a new Pay ID on-chain.
    ///
    /// Both `pay_id_name` and `display_name` are MANDATORY.
    /// Registration fails if display_name is empty or invalid.
    ///
    /// Example:
    ///   pay_id_name  = "salman"       → creates "salman@zbx"
    ///   display_name = "Salman Tyagi" → stored on-chain alongside address
    pub async fn register(&self, req: RegistrationRequest) -> Result<ResolvedPayId, PayIdError> {
        // 1. Validate Pay ID name format
        let parts = parse_pay_id(&req.pay_id_name)?;
        if parts.handle != "zbx" && parts.handle != req.pay_id_name {
            return Err(PayIdError::InvalidFormat(
                "provide only the name part (e.g. 'salman'), not 'salman@zbx'".into()
            ));
        }

        // 2. Validate display_name — MANDATORY
        validate_display_name(&req.display_name)?;

        // 3. Validate address format
        if !req.address.starts_with("0x") || req.address.len() != 42 {
            return Err(PayIdError::InvalidFormat(
                "address must be a valid 0x hex address (42 chars)".into()
            ));
        }

        // 4. In production: eth_sendRawTransaction → ZbxPayId.register(name, displayName, address)
        info!(
            pay_id  = %parts.canonical,
            display = %req.display_name,
            address = %req.address,
            "Pay ID registered"
        );

        Ok(ResolvedPayId {
            input:          req.pay_id_name.clone(),
            pay_id:         parts.canonical,
            display_name:   req.display_name,
            address:        req.address,
            is_smart_wallet: false,
            chain_id:       zbx_types::CHAIN_ID_MAINNET,
        })
    }

    /// Resolve any Pay ID input to a wallet address + display name.
    ///
    /// Accepts: "salman", "salman@zbx", "shop.salman@zbx", "0x742d..." (passthrough)
    pub async fn resolve(&self, input: &str) -> Result<ResolvedPayId, PayIdError> {
        debug!(input, "resolving Pay ID");

        // Raw address passthrough (no display name — address-only lookup)
        if input.starts_with("0x") && input.len() == 42 {
            return Ok(ResolvedPayId {
                input:          input.to_string(),
                pay_id:         input.to_string(),
                display_name:   String::new(),
                address:        input.to_string(),
                is_smart_wallet: false,
                chain_id:       zbx_types::CHAIN_ID_MAINNET,
            });
        }

        let parts = parse_pay_id(input)?;

        // Only resolve ZBX network Pay IDs
        if parts.handle != "zbx" {
            return Err(PayIdError::UnsupportedHandle(parts.handle));
        }

        let (address, display_name) = self.resolve_on_chain(&parts.name).await?;

        info!(
            pay_id  = %parts.canonical,
            display = %display_name,
            address = %address,
            "Pay ID resolved"
        );

        Ok(ResolvedPayId {
            input: input.to_string(),
            pay_id: parts.canonical,
            display_name,
            address,
            is_smart_wallet: false,
            chain_id: zbx_types::CHAIN_ID_MAINNET,
        })
    }

    /// Reverse lookup: address → ("salman@zbx", "Salman Tyagi").
    pub async fn reverse(&self, address: &str) -> Result<Option<(String, String)>, PayIdError> {
        debug!(address, "reverse Pay ID lookup");
        // In production: eth_call ZbxPayId.reverseLookup(address) → (name, displayName)
        let _ = address;
        Ok(None)
    }

    /// Resolve to a specific chain (e.g., ETH or BTC address under same Pay ID).
    pub async fn resolve_chain(&self, input: &str, chain_id: u64) -> Result<String, PayIdError> {
        let parts = parse_pay_id(input)?;
        let _ = (parts, chain_id);
        // In production: eth_call ZbxPayId.resolveChain(name, chainId)
        Err(PayIdError::NotFound(input.to_string()))
    }

    /// Returns (address, display_name) from on-chain contract.
    async fn resolve_on_chain(&self, name: &str) -> Result<(String, String), PayIdError> {
        // In production:
        // 1. ABI-encode resolve(name) → 4-byte selector + encoded string
        // 2. eth_call to self.contract at self.rpc_url
        // 3. Decode returned (address, displayName) tuple
        let _ = name;
        Err(PayIdError::NotFound(format!("{}@zbx", name)))
    }
}