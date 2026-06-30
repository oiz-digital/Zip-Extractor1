//! Block types for ZBX Chain — header, body, builder, validation.

pub mod header;
pub mod body;
pub mod builder;
pub mod validation;
/// N-03 fix (S54): bidirectional conversions between the two canonical block
/// header representations in the workspace.  See module docs for field mapping.
pub mod compat;

pub use header::{BlockHeader, BlockSeal, EMPTY_UNCLE_HASH, EMPTY_TRIE_HASH};
pub use body::{BlockBody, BlobSidecar};
pub use builder::{BlockBuilder, compute_next_base_fee};
pub use validation::{validate_header, validate_body, BlockValidationError};