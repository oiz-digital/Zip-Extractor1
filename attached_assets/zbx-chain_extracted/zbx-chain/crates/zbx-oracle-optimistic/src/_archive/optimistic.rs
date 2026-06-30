//! ZBX Optimistic Oracle -- UMA-style dispute-based data oracle.
//!
//! Unlike Chainlink (push-based, always on), the optimistic oracle is:
//!   PULL-based: a requester asks for data, a proposer answers, then there
//!   is a challenge window. If no dispute: data is accepted. If disputed:
//!   the DVM (Decentralized Verification Module) resolves via ZBX governance.
//!
//! ## Lifecycle of an optimistic oracle request
//!
//!   1. REQUESTER calls request_data(identifier, ancillary_data, bond, reward)
//!      - Deposits reward + bond into contract
//!      - Request is OPEN
//!
//!   2. PROPOSER calls propose_data(request_id, proposed_value)
//!      - Posts a bond (equal to requester bond)
//!      - Starts the liveness window (default: 2 hours)
//!      - Request is PROPOSED
//!
//!   3a. No dispute before liveness expires:
//!       - Anyone calls settle(request_id)
//!       - Proposer gets their bond back + the reward
//!       - Request is SETTLED
//!
//!   3b. DISPUTER calls dispute(request_id, dispute_value)
//!       - Posts a bond
//!       - DVM is asked to vote on the correct answer
//!       - Request is DISPUTED
//!
//!   4.  DVM resolves (via ZBX governance vote):
//!       - If proposer was right: proposer gets both bonds + reward (disputer loses bond)
//!       - If disputer was right: disputer gets both bonds (proposer loses bond + reward)
//!       - Request is RESOLVED

/// Default liveness window (seconds) -- 2 hours.
pub const DEFAULT_LIVENESS_SECS: u64 = 7_200;

/// Optimistic oracle request states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestState {
    Open,       // No proposer yet
    Proposed,   // Proposer submitted, liveness counting
    Settled,    // Accepted without dispute
    Disputed,   // In DVM arbitration
    Resolved,   // DVM returned verdict
    Expired,    // Liveness expired, no proposer -- reward refunded
}

/// An optimistic oracle data request.
#[derive(Debug, Clone)]
pub struct OracleRequest {
    pub id:              [u8; 32],
    pub requester:       [u8; 20],
    pub identifier:      String,    // e.g. "PRICE_REQUEST", "BINARY_ASSERTION"
    pub ancillary_data:  Vec<u8>,   // e.g. JSON with symbol, timestamp, source
    pub timestamp:       u64,
    pub bond:            u128,      // ZBX tokens as collateral
    pub reward:          u128,      // ZBX tokens rewarded to correct proposer
    pub liveness_secs:   u64,
    pub state:           RequestState,
    pub proposed_value:  Option<i128>,
    pub proposed_at:     Option<u64>,
    pub proposer:        Option<[u8; 20]>,
    pub disputer:        Option<[u8; 20]>,
    pub resolved_value:  Option<i128>,
}

impl OracleRequest {
    pub fn new(
        id:             [u8; 32],
        requester:      [u8; 20],
        identifier:     String,
        ancillary_data: Vec<u8>,
        bond:           u128,
        reward:         u128,
        timestamp:      u64,
    ) -> Self {
        Self {
            id, requester, identifier, ancillary_data, timestamp,
            bond, reward, liveness_secs: DEFAULT_LIVENESS_SECS,
            state: RequestState::Open,
            proposed_value: None, proposed_at: None,
            proposer: None, disputer: None, resolved_value: None,
        }
    }

    /// Can be settled without dispute (liveness elapsed, no dispute)?
    pub fn can_settle(&self, now: u64) -> bool {
        self.state == RequestState::Proposed
            && self.proposed_at.map(|t| now >= t + self.liveness_secs).unwrap_or(false)
    }

    /// Is the request in a terminal state?
    pub fn is_terminal(&self) -> bool {
        matches!(self.state, RequestState::Settled | RequestState::Resolved | RequestState::Expired)
    }
}

/// OptimisticOracle -- manages all open requests.
pub struct OptimisticOracle {
    pub requests: std::collections::HashMap<[u8; 32], OracleRequest>,
}

impl OptimisticOracle {
    pub fn new() -> Self { Self { requests: std::collections::HashMap::new() } }

    /// Step 1: Requester opens a data request.
    pub fn request_data(
        &mut self,
        requester:      [u8; 20],
        identifier:     String,
        ancillary_data: Vec<u8>,
        bond:           u128,
        reward:         u128,
        timestamp:      u64,
    ) -> [u8; 32] {
        let id = compute_request_id(&requester, &identifier, timestamp);
        let req = OracleRequest::new(id, requester, identifier, ancillary_data, bond, reward, timestamp);
        self.requests.insert(id, req);
        id
    }

    /// Step 2: Proposer submits a value.
    pub fn propose_data(
        &mut self,
        request_id:     [u8; 32],
        proposer:       [u8; 20],
        proposed_value: i128,
        now:            u64,
    ) -> Result<(), OracleError> {
        let req = self.requests.get_mut(&request_id)
            .ok_or(OracleError::RequestNotFound)?;
        if req.state != RequestState::Open { return Err(OracleError::InvalidState); }
        req.state          = RequestState::Proposed;
        req.proposer       = Some(proposer);
        req.proposed_value = Some(proposed_value);
        req.proposed_at    = Some(now);
        Ok(())
    }

    /// Step 3a: Settle (liveness elapsed, no dispute).
    pub fn settle(&mut self, request_id: [u8; 32], now: u64) -> Result<i128, OracleError> {
        let req = self.requests.get_mut(&request_id)
            .ok_or(OracleError::RequestNotFound)?;
        if !req.can_settle(now) { return Err(OracleError::LivenessNotElapsed); }
        req.state = RequestState::Settled;
        req.proposed_value.ok_or(OracleError::InvalidState)
    }

    /// Step 3b: Dispute (sends to DVM).
    pub fn dispute(
        &mut self,
        request_id: [u8; 32],
        disputer:   [u8; 20],
        now:        u64,
    ) -> Result<(), OracleError> {
        let req = self.requests.get_mut(&request_id)
            .ok_or(OracleError::RequestNotFound)?;
        if req.state != RequestState::Proposed { return Err(OracleError::InvalidState); }
        let proposed_at = req.proposed_at.unwrap_or(0);
        if now >= proposed_at + req.liveness_secs { return Err(OracleError::LivenessElapsed); }
        req.state    = RequestState::Disputed;
        req.disputer = Some(disputer);
        Ok(())
    }

    /// Step 4: DVM resolves a dispute.
    pub fn resolve(
        &mut self,
        request_id:     [u8; 32],
        resolved_value: i128,
    ) -> Result<(), OracleError> {
        let req = self.requests.get_mut(&request_id)
            .ok_or(OracleError::RequestNotFound)?;
        if req.state != RequestState::Disputed { return Err(OracleError::InvalidState); }
        req.state          = RequestState::Resolved;
        req.resolved_value = Some(resolved_value);
        Ok(())
    }
}

#[derive(Debug)]
pub enum OracleError {
    RequestNotFound, InvalidState, LivenessNotElapsed, LivenessElapsed,
}

fn compute_request_id(requester: &[u8; 20], identifier: &str, timestamp: u64) -> [u8; 32] {
    let _ = (requester, identifier, timestamp);
    [0u8; 32] // stub: keccak256(requester ++ identifier ++ timestamp)
}