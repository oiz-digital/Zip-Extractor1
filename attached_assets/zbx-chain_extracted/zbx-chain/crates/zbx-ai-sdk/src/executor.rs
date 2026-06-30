//! Session Key Transaction Executor (ZEP-017).
//!
//! Agents use session keys to execute on-chain transactions without requiring
//! the owner's private key to be online. Session keys are:
//! - Limited in scope (specific contracts / methods only)
//! - Time-limited (expire after a set number of blocks)
//! - Value-limited (max ZBX per transaction)
//! - Revocable at any time by the owner
//!
//! Security:
//! - All requests are signed with the session key before broadcast
//! - Nonce prevents replay attacks
//! - Value cap prevents draining wallets
//! - Rate limit enforced at EVM level (see ZEP-017)

use crate::{
    strategy::StrategyAction,
    error::SdkError,
};
use serde::{Serialize, Deserialize};

/// Maximum ZBX value per session key transaction (100 ZBX in fp18 = wei).
pub const MAX_TX_VALUE_WEI: u128 = 100 * 1_000_000_000_000_000_000u128;

/// Session key state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionKey {
    /// Session key address (derived from session keypair).
    pub address:        [u8; 20],
    /// Owner address (grants permissions).
    pub owner:          [u8; 20],
    /// Block number when this key expires.
    pub expires_at:     u64,
    /// Max value per transaction (in wei).
    pub max_value_wei:  u128,
    /// Allowed contract addresses (empty = any).
    pub allowed_to:     Vec<[u8; 20]>,
    /// Current nonce.
    pub nonce:          u64,
    /// Whether this key is revoked.
    pub revoked:        bool,
}

impl SessionKey {
    pub fn new(
        address:       [u8; 20],
        owner:         [u8; 20],
        expires_at:    u64,
        max_value_wei: u128,
    ) -> Self {
        Self {
            address,
            owner,
            expires_at,
            max_value_wei: max_value_wei.min(MAX_TX_VALUE_WEI),
            allowed_to:    vec![],
            nonce:         0,
            revoked:       false,
        }
    }

    pub fn is_valid(&self, current_block: u64) -> bool {
        !self.revoked && current_block <= self.expires_at
    }

    pub fn is_contract_allowed(&self, target: &[u8; 20]) -> bool {
        self.allowed_to.is_empty() || self.allowed_to.contains(target)
    }

    pub fn revoke(&mut self) { self.revoked = true; }
}

/// A pending action request to be executed via session key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionRequest {
    pub action:        StrategyAction,
    pub session_key:   [u8; 20],
    pub target:        [u8; 20],
    pub calldata:      Vec<u8>,
    pub value_wei:     u128,
    pub nonce:         u64,
    pub block_number:  u64,
}

/// Receipt for an executed action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionReceipt {
    pub request_id:   [u8; 32],
    pub tx_hash:      [u8; 32],
    pub block_number: u64,
    pub gas_used:     u64,
    pub success:      bool,
    pub error:        Option<String>,
}

impl ActionReceipt {
    fn compute_id(req: &ActionRequest) -> [u8; 32] {
        use sha3::{Digest, Sha3_256};
        let mut h = Sha3_256::new();
        h.update(&req.session_key);
        h.update(&req.nonce.to_be_bytes());
        h.update(&req.block_number.to_be_bytes());
        h.update(&req.calldata);
        let out = h.finalize();
        let mut id = [0u8; 32];
        id.copy_from_slice(&out);
        id
    }
}

/// Session Key Executor — validates and (in production) broadcasts transactions.
pub struct SessionKeyExecutor {
    session_key:   SessionKey,
    current_block: u64,
    /// Simulated receipts (in production this submits to the node RPC).
    pending:       Vec<ActionRequest>,
}

impl SessionKeyExecutor {
    pub fn new(session_key: SessionKey, current_block: u64) -> Self {
        Self { session_key, current_block, pending: vec![] }
    }

    /// Validate and queue an action for execution.
    pub fn submit(&mut self, action: StrategyAction) -> Result<ActionRequest, SdkError> {
        // Check session key validity
        if !self.session_key.is_valid(self.current_block) {
            return Err(SdkError::SessionKeyExpired {
                expires_at:    self.session_key.expires_at,
                current_block: self.current_block,
            });
        }

        // Value cap check
        let value_wei = fp6_to_wei(action.amount_fp6);
        if value_wei > self.session_key.max_value_wei {
            return Err(SdkError::SessionKeyValueExceeded {
                requested: value_wei,
                max:       self.session_key.max_value_wei,
            });
        }

        // Encode calldata (simplified ABI encoding)
        let calldata = encode_action_calldata(&action);
        let target   = action_target(&action);

        if !self.session_key.is_contract_allowed(&target) {
            return Err(SdkError::SessionKeyContractNotAllowed {
                target: hex_encode_20(&target),
            });
        }

        let nonce = self.session_key.nonce;
        self.session_key.nonce += 1;

        let req = ActionRequest {
            action,
            session_key:  self.session_key.address,
            target,
            calldata,
            value_wei,
            nonce,
            block_number: self.current_block,
        };

        tracing::info!(
            nonce        = req.nonce,
            block        = req.block_number,
            value_wei    = req.value_wei,
            kind         = ?req.action.kind,
            "Session key action submitted"
        );

        self.pending.push(req.clone());
        Ok(req)
    }

    /// Drain pending actions and simulate receipts.
    pub fn flush(&mut self) -> Vec<ActionReceipt> {
        self.pending.drain(..).map(|req| {
            let id = ActionReceipt::compute_id(&req);
            // Deterministic stub tx_hash from request_id
            let mut tx_hash = id;
            tx_hash[0] ^= 0xAB; // differentiate from request_id
            ActionReceipt {
                request_id:   id,
                tx_hash,
                block_number: req.block_number,
                gas_used:     150_000,
                success:      true,
                error:        None,
            }
        }).collect()
    }

    pub fn advance_block(&mut self, block: u64) {
        self.current_block = block;
    }
}

/// Convert fixed-point-6 to wei (multiply by 10^12 to go fp6 → fp18).
fn fp6_to_wei(fp6: u64) -> u128 {
    fp6 as u128 * 1_000_000_000_000u128
}

/// Encode action as minimal calldata bytes.
fn encode_action_calldata(action: &StrategyAction) -> Vec<u8> {
    use crate::strategy::ActionKind;
    let mut buf = Vec::new();
    match &action.kind {
        ActionKind::Swap { from_token, to_token } => {
            buf.push(0x01); // swap selector
            buf.extend_from_slice(from_token.as_bytes());
            buf.push(0x00);
            buf.extend_from_slice(to_token.as_bytes());
            buf.push(0x00);
            buf.extend_from_slice(&action.amount_fp6.to_be_bytes());
        }
        ActionKind::AddLiquidity { pool } => {
            buf.push(0x02);
            buf.extend_from_slice(pool.as_bytes());
            buf.extend_from_slice(&action.amount_fp6.to_be_bytes());
        }
        ActionKind::RemoveLiquidity { pool } => {
            buf.push(0x03);
            buf.extend_from_slice(pool.as_bytes());
            buf.extend_from_slice(&action.amount_fp6.to_be_bytes());
        }
        ActionKind::Stake => {
            buf.push(0x04);
            buf.extend_from_slice(&action.amount_fp6.to_be_bytes());
        }
        ActionKind::Unstake => {
            buf.push(0x05);
            buf.extend_from_slice(&action.amount_fp6.to_be_bytes());
        }
        ActionKind::Alert { message } => {
            buf.push(0x06);
            buf.extend_from_slice(message.as_bytes());
        }
        ActionKind::NoOp => { buf.push(0x00); }
    }
    buf
}

/// Derive target contract address from action kind.
fn action_target(action: &StrategyAction) -> [u8; 20] {
    use crate::strategy::ActionKind;
    match &action.kind {
        ActionKind::Swap { .. }           => [0x01; 20], // ZBX DEX router
        ActionKind::AddLiquidity { .. }
        | ActionKind::RemoveLiquidity {..} => [0x02; 20], // LP manager
        ActionKind::Stake
        | ActionKind::Unstake             => [0x03; 20], // staking contract
        ActionKind::Alert { .. }          => [0x00; 20], // no target (log only)
        ActionKind::NoOp                  => [0x00; 20],
    }
}

fn hex_encode_20(b: &[u8; 20]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::{StrategyAction, ActionKind};

    fn session_key() -> SessionKey {
        SessionKey::new([0xABu8; 20], [0x01u8; 20], 999_999, MAX_TX_VALUE_WEI)
    }

    fn swap_action() -> StrategyAction {
        StrategyAction {
            kind:       ActionKind::Swap { from_token: "USDT".to_string(), to_token: "ZBX".to_string() },
            pair:       "ZBX/USDT".to_string(),
            amount_fp6: 100_000_000,
            reason:     "test swap".to_string(),
            priority:   5,
        }
    }

    #[test]
    fn valid_session_key_submits_action() {
        let mut exec = SessionKeyExecutor::new(session_key(), 1000);
        let req = exec.submit(swap_action()).unwrap();
        assert_eq!(req.nonce, 0);
        assert!(req.calldata[0] == 0x01); // swap selector
    }

    #[test]
    fn expired_session_key_rejects() {
        let key = SessionKey::new([0xABu8; 20], [0x01u8; 20], 100, MAX_TX_VALUE_WEI);
        let mut exec = SessionKeyExecutor::new(key, 200); // block 200 > expires 100
        let err = exec.submit(swap_action()).unwrap_err();
        assert!(matches!(err, SdkError::SessionKeyExpired { .. }));
    }

    #[test]
    fn nonce_increments() {
        let mut exec = SessionKeyExecutor::new(session_key(), 1);
        exec.submit(swap_action()).unwrap();
        exec.submit(swap_action()).unwrap();
        assert_eq!(exec.session_key.nonce, 2);
    }

    #[test]
    fn flush_returns_receipts() {
        let mut exec = SessionKeyExecutor::new(session_key(), 1);
        exec.submit(swap_action()).unwrap();
        exec.submit(swap_action()).unwrap();
        let receipts = exec.flush();
        assert_eq!(receipts.len(), 2);
        assert!(receipts.iter().all(|r| r.success));
        assert!(exec.pending.is_empty());
    }
}
