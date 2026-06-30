//! Optimistic Oracle for ZBX chain (ZEP-012).
//!
//! # How It Works
//!
//! Unlike Chainlink (always on, push-based), optimistic oracle is:
//!   - Pull-based: data only fetched when someone requests it
//!   - General-purpose: can answer ANY yes/no or numeric question
//!   - Dispute-driven: anyone can challenge a wrong answer
//!
//! # Flow
//!
//! ```
//! 1. Requester: "What was ETH price at block 19000000?"
//!    Bond: 10 ZBX (pays for oracle service)
//!
//! 2. Proposer: "It was $3,542.18"
//!    Bond: 50 ZBX (skin in the game)
//!
//! 3. Challenge window: 2 hours
//!    a) No challenge → price accepted, proposer gets bond back + reward
//!    b) Challenger disputes → DVM (Dispute Voting Mechanism) activated
//!
//! 4. DVM (if dispute):
//!    ZBX token holders vote on correct answer
//!    Wrong party loses bond → given to correct party
//! ```
//!
//! # Use Cases
//!
//! - Historical prices (audits, settlements, derivatives expiry)
//! - Event outcomes ("Did ETH reach $5000 in Jan 2025?")
//! - Real-world data (flight delays, election results, sports scores)
//! - Custom DeFi triggers ("Liquidate if ZUSD depeg > 5% for 1 hour")
//!
//! # Comparison
//!
//! | Feature        | Chainlink  | ZBX Optimistic |
//! |:---|:---|:---|
//! | Data type      | Prices only | Anything       |
//! | Cost           | Always paid | Only if used   |
//! | Finality       | Immediate   | 2h delay       |
//! | Dispute        | None        | DVM vote       |
//! | Bond required  | No          | Yes (50 ZBX)   |

pub mod request;
pub mod proposal;
pub mod dispute;
pub mod dvm;

pub use request::{OracleRequest, RequestId, AncillaryData};
pub use proposal::PriceProposal;
pub use dispute::{Dispute, DisputeOutcome};
pub use dvm::DisputeVotingMechanism;