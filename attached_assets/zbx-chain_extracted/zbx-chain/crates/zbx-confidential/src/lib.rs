//! zbx-confidential — Confidential Transactions for ZBX Chain (ZEP-025).
//!
//! Implements optional transaction privacy using:
//! - **Pedersen commitments**: hide ZRC-20 transfer amounts
//! - **Bulletproofs range proofs**: prove committed amounts are non-negative
//! - **Stealth addresses**: hide recipient identity (ERC-5564 compatible)
//! - **Balance conservation**: prove no tokens created/destroyed
//!
//! ## Design Principles
//!
//! 1. **Opt-in privacy**: Default transactions are transparent.
//!    Confidential mode is explicitly requested.
//!
//! 2. **Selective disclosure**: Users can reveal transactions to auditors
//!    by sharing blinding factors or view keys.
//!
//! 3. **Gas stays transparent**: ZBX/ZUSD gas fees are always visible.
//!    Only ZRC-20 token amounts are hidden.
//!
//! 4. **Compliance-ready**: View keys allow auditors to scan all received
//!    transactions without spending authority.
//!
//! ## Usage
//!
//! ```rust,no_run
//! use zbx_confidential::{commitment, stealth, range_proof};
//! use zbx_confidential::commitment::{PedersenCommitment, BlindingFactor};
//! use rand::rngs::OsRng;
//!
//! // Commit to an amount
//! let blinding = BlindingFactor::random(&mut OsRng);
//! let commitment = PedersenCommitment::commit(1_000_000, &blinding);
//!
//! // Prove the amount is valid (0 ≤ v < 2^64)
//! let proof = range_proof::prove_range(1_000_000, &blinding);
//! range_proof::verify_range(&proof).unwrap();
//!
//! // Generate a stealth address for the recipient
//! let recipient_keys = stealth::StealthRecipientKeys { spend_key: [1u8;32], view_key: [2u8;32] };
//! let meta = recipient_keys.meta_address();
//! let (stealth_addr, _) = stealth::generate_stealth_address(&meta, &mut OsRng).unwrap();
//! ```

pub mod commitment;
pub mod error;
pub mod range_proof;
pub mod stealth;

pub use commitment::{
    BlindingFactor, CommitmentOpening, PedersenCommitment,
    verify_balance_conservation,
};
pub use error::ConfidentialError;
pub use range_proof::{RangeProof, RANGE_BITS, batch_verify_range, prove_range, verify_range};
pub use stealth::{
    ReceivedPayment, StealthAddress, StealthMetaAddress, StealthRecipientKeys,
    generate_stealth_address, scan_tx_for_recipient,
};
