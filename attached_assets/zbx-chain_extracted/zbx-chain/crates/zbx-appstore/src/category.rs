//! App category taxonomy for the Zebvix App Store.

use serde::{Deserialize, Serialize};
use std::fmt;

/// All supported application categories.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppCategory {
    /// Wallet apps (key management, signing, payment UX).
    Wallet,
    /// Decentralised finance (DEX, lending, yield, perps).
    Defi,
    /// NFT minting, marketplace, and gallery apps.
    Nft,
    /// AI-powered tools running on or off chain.
    AiTools,
    /// Blockchain games (on-chain game logic or ZVM-based).
    Games,
    /// Miscellaneous utilities (explorers, bridges, analytics).
    Utilities,
}

impl AppCategory {
    /// Return the canonical slug used as a storage key prefix.
    pub fn slug(&self) -> &'static str {
        match self {
            AppCategory::Wallet   => "wallet",
            AppCategory::Defi     => "defi",
            AppCategory::Nft      => "nft",
            AppCategory::AiTools  => "ai_tools",
            AppCategory::Games    => "games",
            AppCategory::Utilities => "utilities",
        }
    }

    /// All categories in display order.
    pub fn all() -> &'static [AppCategory] {
        use AppCategory::*;
        &[Wallet, Defi, Nft, AiTools, Games, Utilities]
    }

    /// Parse from a slug string.
    pub fn from_slug(s: &str) -> Option<Self> {
        match s {
            "wallet"    => Some(AppCategory::Wallet),
            "defi"      => Some(AppCategory::Defi),
            "nft"       => Some(AppCategory::Nft),
            "ai_tools"  => Some(AppCategory::AiTools),
            "games"     => Some(AppCategory::Games),
            "utilities" => Some(AppCategory::Utilities),
            _           => None,
        }
    }
}

impl fmt::Display for AppCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.slug())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_slug() {
        for cat in AppCategory::all() {
            let slug = cat.slug();
            let parsed = AppCategory::from_slug(slug)
                .unwrap_or_else(|| panic!("failed to parse slug '{}'", slug));
            assert_eq!(&parsed, cat);
        }
    }

    #[test]
    fn unknown_slug_returns_none() {
        assert!(AppCategory::from_slug("unknown_category").is_none());
    }
}
