//! zbx-nft — NFT standard (ZEP-721 / ZEP-1155) implementation.
//!
//! Covers minting, batch transfers, metadata URI, royalty enforcement
//! (EIP-2981 compatible), and the native marketplace order book.

pub mod mint;
pub mod transfer;
pub mod metadata;
pub mod royalty;
pub mod marketplace;