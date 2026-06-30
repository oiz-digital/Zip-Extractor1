//! zbx-payid — UPI-style Pay ID resolver for ZBX Chain.
//!
//! Pay ID format (exactly like UPI):
//!   ali@zbx            ← standard (like paytm@upi or 9876543210@okaxis)
//!   shop.ali@zbx       ← sub-ID issued by "ali" to their shop branch
//!
//! Resolution:
//!   "ali@zbx"       → 0x742d35Cc6634C0532925a3b844Bc454e4438f44e
//!   "shop.ali@zbx"  → 0xAnotherAddress...
//!   "ali"           → same (suffix optional inside ZBX network)
//!
//! Multi-chain:
//!   ali@zbx on chain 1 (ETH)  → 0xSameOrDifferentEthAddress
//!   ali@zbx BTC               → bc1qxy2kgdygjrsqtzq2n0yrf...

pub mod resolver;
pub mod parser;
pub mod registry;
pub mod error;

pub use resolver::{PayIdResolver, ResolvedPayId, RegistrationRequest};
pub use parser::{parse_pay_id, validate_display_name, PayIdParts};
pub use registry::PayIdRegistry;
pub use error::PayIdError;

/// ZBX Pay ID contract address on mainnet.
pub const PAYID_CONTRACT: &str = "0x7e4a7f8bCE8CfD1765CdE34Dbf5e8D7fB1A43e9";

/// The ZBX network handle — the part after @.
pub const ZBX_HANDLE: &str = "zbx";

/// Full suffix used in display.
pub const ZBX_SUFFIX: &str = "@zbx";

/// Format a bare name into full UPI-style Pay ID.
pub fn format_pay_id(name: &str) -> String {
    format!("{}@zbx", name.to_lowercase())
}