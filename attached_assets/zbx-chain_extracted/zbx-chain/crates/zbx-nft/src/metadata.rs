//! NFT metadata URI storage (on-chain / IPFS pointer).

use std::collections::HashMap;
use crate::mint::TokenId;

/// Per-token metadata URI record.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TokenMetadata {
    /// URI pointing to the JSON metadata (e.g. "ipfs://Qm...").
    pub uri:         String,
    /// Optional on-chain name override.
    pub name:        Option<String>,
    /// Optional on-chain description override.
    pub description: Option<String>,
}

/// Collection-level metadata store.
#[derive(Debug, Default)]
pub struct MetadataStore {
    records:      HashMap<TokenId, TokenMetadata>,
    base_uri:     Option<String>,
    frozen:       bool,
}

impl MetadataStore {
    pub fn new() -> Self { Self::default() }

    /// Set a shared base URI; `token_uri()` appends the token ID.
    pub fn set_base_uri(&mut self, base: String) -> Result<(), &'static str> {
        if self.frozen { return Err("metadata frozen"); }
        self.base_uri = Some(base);
        Ok(())
    }

    pub fn set_token_uri(&mut self, token_id: TokenId, uri: String) -> Result<(), &'static str> {
        if self.frozen { return Err("metadata frozen"); }
        let rec = self.records.entry(token_id).or_insert(TokenMetadata {
            uri: String::new(), name: None, description: None,
        });
        rec.uri = uri;
        Ok(())
    }

    pub fn token_uri(&self, token_id: TokenId) -> Option<String> {
        if let Some(rec) = self.records.get(&token_id) {
            if !rec.uri.is_empty() { return Some(rec.uri.clone()); }
        }
        self.base_uri.as_ref().map(|b| format!("{}{}", b, token_id))
    }

    /// Freeze metadata permanently (no further changes allowed).
    pub fn freeze(&mut self) { self.frozen = true; }

    pub fn is_frozen(&self) -> bool { self.frozen }
}
