//! Oracle request — someone asking for data.

use serde::{Serialize, Deserialize};

pub type RequestId = [u8; 32];

/// Arbitrary data describing what is being asked.
/// e.g. b"ETH/USD at block 19000000"
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AncillaryData(pub Vec<u8>);

impl AncillaryData {
    pub fn price_at_block(symbol: &str, block: u64) -> Self {
        Self(format!("{symbol} price at block {block}").into_bytes())
    }
    pub fn event_outcome(description: &str) -> Self {
        Self(description.as_bytes().to_vec())
    }
}

/// The state of an oracle request.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum RequestState {
    /// Request created, awaiting a proposal
    Pending,
    /// A proposal has been submitted, in challenge window
    Proposed { proposed_at: u64 },
    /// Challenge window passed, price accepted
    Settled { final_price: i128 },
    /// Dispute raised, DVM vote in progress
    Disputed,
    /// Dispute resolved by DVM
    DisputeResolved { final_price: i128, winner: [u8; 20] },
}

/// An oracle request.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OracleRequest {
    pub id:                RequestId,
    /// Who is asking
    pub requester:         [u8; 20],
    /// What are they asking
    pub ancillary_data:    AncillaryData,
    /// ZBX reward for correct answer
    pub reward_zbx:        u128,
    /// Required proposer bond (ZBX)
    pub bond_zbx:          u128,
    /// Challenge window duration (seconds)
    pub challenge_window:  u64,
    /// Current state
    pub state:             RequestState,
    /// Block timestamp when request was created
    pub created_at:        u64,
}

impl OracleRequest {
    pub fn new(
        requester:        [u8; 20],
        ancillary_data:   AncillaryData,
        reward_zbx:       u128,
        bond_zbx:         u128,
        challenge_window: u64,
        now:              u64,
    ) -> (RequestId, Self) {
        use sha2::{Sha256, Digest};
        let mut h = Sha256::new();
        h.update(&requester);
        h.update(&ancillary_data.0);
        h.update(&now.to_le_bytes());
        let id: [u8; 32] = h.finalize().into();
        let req = Self {
            id, requester, ancillary_data,
            reward_zbx, bond_zbx, challenge_window,
            state: RequestState::Pending,
            created_at: now,
        };
        (id, req)
    }

    /// Can this request accept a proposal?
    pub fn is_pending(&self) -> bool { self.state == RequestState::Pending }

    /// Is the challenge window still open?
    pub fn in_challenge_window(&self, now: u64) -> bool {
        if let RequestState::Proposed { proposed_at } = self.state {
            now < proposed_at + self.challenge_window
        } else { false }
    }

    /// Can this be settled (challenge window expired, no dispute)?
    pub fn can_settle(&self, now: u64) -> bool {
        if let RequestState::Proposed { proposed_at } = self.state {
            now >= proposed_at + self.challenge_window
        } else { false }
    }
}