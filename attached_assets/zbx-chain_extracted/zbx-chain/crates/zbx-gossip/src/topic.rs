//! GossipSub topic definitions for Zebvix Chain.

use std::fmt;

/// A gossip topic identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Topic(String);

impl Topic {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str { &self.0 }
}

impl fmt::Display for Topic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Well-known topics for Zebvix Chain.
pub struct Topics;

impl Topics {
    /// Mempool: pending transactions.
    pub fn mempool(chain_id: u64) -> Topic {
        Topic::new(format!("/zbx/{}/mempool/1.0.0", chain_id))
    }
    /// Consensus: block proposals.
    pub fn proposals(chain_id: u64) -> Topic {
        Topic::new(format!("/zbx/{}/proposals/1.0.0", chain_id))
    }
    /// Consensus: votes.
    pub fn votes(chain_id: u64) -> Topic {
        Topic::new(format!("/zbx/{}/votes/1.0.0", chain_id))
    }
    /// Block announcements.
    pub fn blocks(chain_id: u64) -> Topic {
        Topic::new(format!("/zbx/{}/blocks/1.0.0", chain_id))
    }
}