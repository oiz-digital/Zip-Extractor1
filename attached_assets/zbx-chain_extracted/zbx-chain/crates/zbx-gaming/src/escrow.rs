//! Game session escrow — off-chain state mirror for ZbxGameEscrow.sol.
//!
//! Used by game servers to track session state without reading chain storage
//! on every request, and by CLI tooling to display open/active sessions.

use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use zbx_types::address::Address;

/// U256-compatible stake amount (stored as [u8; 32]).
pub type Amount = [u8; 32];

/// Session states, mirroring the Solidity enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionState {
    Open,
    Active,
    Resolved,
    Cancelled,
}

/// An escrow session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameSession {
    pub session_id:    [u8; 32],
    pub player_a:      Address,
    pub player_b:      Option<Address>,
    pub game_contract: Address,
    /// ERC-20 token address, or zero for native ZBX.
    pub token:         Address,
    pub stake:         Amount,
    pub created_block: u64,
    pub state:         SessionState,
}

/// In-memory index of game sessions.
#[derive(Debug, Default)]
pub struct EscrowIndex {
    sessions: HashMap<[u8; 32], GameSession>,
}

impl EscrowIndex {
    pub fn new() -> Self { Self::default() }

    pub fn insert(&mut self, session: GameSession) {
        self.sessions.insert(session.session_id, session);
    }

    pub fn get(&self, id: &[u8; 32]) -> Option<&GameSession> {
        self.sessions.get(id)
    }

    pub fn get_mut(&mut self, id: &[u8; 32]) -> Option<&mut GameSession> {
        self.sessions.get_mut(id)
    }

    /// Open sessions waiting for a second player.
    pub fn open_sessions(&self) -> Vec<&GameSession> {
        self.sessions.values()
            .filter(|s| s.state == SessionState::Open)
            .collect()
    }

    /// Active sessions being played.
    pub fn active_sessions(&self) -> Vec<&GameSession> {
        self.sessions.values()
            .filter(|s| s.state == SessionState::Active)
            .collect()
    }

    /// Sessions belonging to a specific player (any state).
    pub fn sessions_for_player(&self, player: &Address) -> Vec<&GameSession> {
        self.sessions.values()
            .filter(|s| &s.player_a == player
                || s.player_b.as_ref() == Some(player))
            .collect()
    }

    /// Update a session's state (called when processing chain events).
    pub fn update_state(&mut self, id: &[u8; 32], state: SessionState) {
        if let Some(s) = self.sessions.get_mut(id) {
            s.state = state;
        }
    }

    /// Record player B joining a session.
    pub fn set_player_b(&mut self, id: &[u8; 32], player_b: Address) {
        if let Some(s) = self.sessions.get_mut(id) {
            s.player_b = Some(player_b);
            s.state    = SessionState::Active;
        }
    }
}
