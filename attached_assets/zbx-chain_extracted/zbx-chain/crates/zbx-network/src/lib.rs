//! zbx-network: P2P networking for Zebvix.
//!
//! ## Subsystems
//!
//! | Module | Responsibility |
//! |--------|----------------|
//! | `transport` | Framed TCP with 4-byte length prefix (Noise XX encrypted) |
//! | `messages` | Typed message enum covering all protocol message types |
//! | `peer` | Peer identity, state, and connection lifecycle |
//! | `discovery` | Kademlia-based peer routing table (256 k-buckets, k=16) |
//! | `gossip` | Gossip fan-out: seen-message dedup, topic subscriptions |
//! | `peer_score` | Peer reputation scoring and ban management |

pub mod discovery;
pub mod error;
pub mod gossip;
pub mod messages;
pub mod peer;
pub mod peer_score;
pub mod peer_store;
pub mod transport;

pub use error::NetworkError;
pub use messages::{
    Message, MessageType, StatusMessage, GetBlockRange,
    Hs2ProposalMessage, GossipEnvelope,
};
pub use peer::{PeerId, PeerInfo, PeerManager};
pub use gossip::{GossipRouter, GossipMessage, GossipTopic, GossipDecision, Subscriptions,
    MAX_SEEN_MESSAGES, MAX_HOPS, DEFAULT_FANOUT};
pub use peer_score::{PeerScorer, PeerScore, ScorePenalty, BAN_THRESHOLD, INITIAL_SCORE,
    DECAY_INTERVAL, MAX_SCORE, MIN_SCORE};
pub use peer_store::{PeerStore, PersistentBan, DEFAULT_BAN_TTL_SECS,
    MAX_BANLIST_ENTRIES, MAX_PEER_STORE_ENTRIES};