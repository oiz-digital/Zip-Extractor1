//! zbx-consensus: HotStuff-based BFT consensus for Zebvix.
//!
//! ## Protocol versions
//!
//! | Module | Protocol | Status |
//! |--------|----------|--------|
//! | `hotstuff` | 3-phase HotStuff (Prepare → PreCommit → Commit) | Production |
//! | `hotstuff2` | HotStuff-2 + Jolteon view change (ZEP-022) | Production |
//! | `epoch_manager` | Validator set rotation (per-epoch) | Production |
//! | `proposer` | VRF-based leader election | Production |
//! | `safety_rules` | WAL-persisted equivocation prevention | Production |

pub mod block_store;
pub mod epoch_manager;
pub mod error;
pub mod finality;
pub mod gossip;
pub mod hotstuff;
pub mod hotstuff2;
pub mod liveness;
pub mod pacemaker;
pub mod proposer;
pub mod round_manager;
pub mod safety_rules;
pub mod vote;

pub mod slashing {
    pub mod inactivity;
}

pub use error::ConsensusError;
pub use gossip::{GossipEngine, GossipMessage, GossipPriority, Envelope};
pub use hotstuff::{HotStuffConsensus, Phase};
pub use liveness::Pacemaker;
pub use pacemaker::{
    PacemakerCoordinator, CoordinatorEvent,
    TimeoutShare as PacemakerTimeoutShare,
    TimeoutCertificate as PacemakerTc,
    TimeoutShareData,
};
pub use round_manager::{RoundManager, RoundState};
pub use safety_rules::SafetyRules;
pub use hotstuff2::{
    AdaptiveTimer, HotStuff2, Hs2Event, Hs2Phase,
    TcAccumulator, TimeoutCertificate, TimeoutShare,
    genesis_qc,
    DELTA_INIT, DELTA_MAX, DELTA_MIN, MAX_CONSECUTIVE_TIMEOUTS,
};
pub use vote::{Vote, VoteData, QuorumCertificate, EquivocationEvidence};
pub use epoch_manager::{
    EpochManager, EpochState, EpochEvent, ValidatorEntry,
    EPOCH_LENGTH, MAX_VALIDATORS, MIN_VALIDATOR_STAKE,
};
pub use proposer::ProposerElection;
pub use finality::{Checkpoint, FinalityTracker, Justification};