//! TimelockController — queues governance operations with a mandatory delay.
//!
//! Any Succeeded proposal must be queued here before execution.
//! The guardian can veto (cancel) any queued operation before it executes.
//! Minimum delay: 2 days. Maximum delay: 30 days.

use std::collections::HashMap;
use sha3::{Digest, Sha3_256};
use zbx_types::address::Address;

/// Minimum enforced delay between queue and execute (2 days).
pub const MIN_DELAY_SECS: u64 = 2 * 24 * 3600;
/// Maximum allowed delay (30 days).
pub const MAX_DELAY_SECS: u64 = 30 * 24 * 3600;

/// A single executable call inside an operation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Call {
    /// Target contract address.
    pub target: Address,
    /// Encoded calldata (ABI-encoded function selector + args).
    pub calldata: Vec<u8>,
    /// Optional native ZBX value (wei) to forward.
    pub value: u128,
}

/// State of a queued operation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum OperationState {
    /// Queued and waiting for eta to pass.
    Pending { eta: u64 },
    /// Ready — eta has passed, can be executed.
    Ready,
    /// Already executed.
    Done,
    /// Cancelled by guardian veto or proposer.
    Cancelled,
}

/// A queued governance operation (one or many calls batched together).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TimelockOperation {
    /// SHA3-256 hash of (calls ‖ salt ‖ predecessor).
    pub id: [u8; 32],
    pub calls: Vec<Call>,
    /// Optional predecessor operation that must execute first.
    pub predecessor: Option<[u8; 32]>,
    pub salt: [u8; 32],
    pub state: OperationState,
    pub proposer: Address,
}

/// Timelock error type.
#[derive(Debug, thiserror::Error)]
pub enum TimelockError {
    #[error("operation already queued: {0}")]
    AlreadyQueued(String),
    #[error("operation not found: {0}")]
    NotFound(String),
    #[error("operation not ready — eta not reached (now={now}, eta={eta})")]
    NotReady { now: u64, eta: u64 },
    #[error("predecessor not executed: {0}")]
    PredecessorPending(String),
    #[error("operation not in pending state")]
    NotPending,
    #[error("operation already done")]
    AlreadyDone,
    #[error("operation cancelled")]
    Cancelled,
    #[error("caller is not guardian")]
    NotGuardian,
    #[error("caller is not admin")]
    NotAdmin,
    #[error("delay {delay} out of range [{min}, {max}]")]
    InvalidDelay { delay: u64, min: u64, max: u64 },
}

/// TimelockController — enforces a mandatory delay on all governance executions.
#[derive(Debug)]
pub struct TimelockController {
    operations: HashMap<[u8; 32], TimelockOperation>,
    /// Delay in seconds (MIN_DELAY_SECS ≤ delay ≤ MAX_DELAY_SECS).
    pub delay: u64,
    /// Guardian address — may cancel any pending operation (veto power).
    pub guardian: Address,
    /// Admin — may update delay and guardian.
    pub admin: Address,
}

impl TimelockController {
    /// Create a new controller with the given delay (seconds).
    pub fn new(delay: u64, guardian: Address, admin: Address) -> Result<Self, TimelockError> {
        if delay < MIN_DELAY_SECS || delay > MAX_DELAY_SECS {
            return Err(TimelockError::InvalidDelay {
                delay,
                min: MIN_DELAY_SECS,
                max: MAX_DELAY_SECS,
            });
        }
        Ok(Self { operations: HashMap::new(), delay, guardian, admin })
    }

    /// Compute the deterministic operation ID from calls, predecessor, and salt.
    pub fn hash_operation(
        calls: &[Call],
        predecessor: Option<[u8; 32]>,
        salt: [u8; 32],
    ) -> [u8; 32] {
        let mut h = Sha3_256::new();
        for call in calls {
            h.update(call.target.0);
            h.update(&call.calldata);
            h.update(call.value.to_be_bytes());
        }
        if let Some(pred) = predecessor {
            h.update(pred);
        } else {
            h.update([0u8; 32]);
        }
        h.update(salt);
        h.finalize().into()
    }

    /// Queue an operation. `now` is the current block timestamp (seconds).
    pub fn schedule(
        &mut self,
        proposer: Address,
        calls: Vec<Call>,
        predecessor: Option<[u8; 32]>,
        salt: [u8; 32],
        now: u64,
    ) -> Result<[u8; 32], TimelockError> {
        let id = Self::hash_operation(&calls, predecessor, salt);
        let id_hex = hex::encode(id);
        if self.operations.contains_key(&id) {
            return Err(TimelockError::AlreadyQueued(id_hex));
        }
        let eta = now + self.delay;
        self.operations.insert(id, TimelockOperation {
            id,
            calls,
            predecessor,
            salt,
            state: OperationState::Pending { eta },
            proposer,
        });
        Ok(id)
    }

    /// Execute a ready operation. Returns the list of calls to be dispatched.
    pub fn execute(
        &mut self,
        id: [u8; 32],
        now: u64,
    ) -> Result<Vec<Call>, TimelockError> {
        let op = self.operations.get(&id)
            .ok_or_else(|| TimelockError::NotFound(hex::encode(id)))?;

        match &op.state {
            OperationState::Pending { eta } => {
                let eta = *eta;
                if now < eta {
                    return Err(TimelockError::NotReady { now, eta });
                }
                // Check predecessor
                if let Some(pred) = op.predecessor {
                    let pred_done = self.operations.get(&pred)
                        .map(|p| p.state == OperationState::Done)
                        .unwrap_or(false);
                    if !pred_done {
                        return Err(TimelockError::PredecessorPending(hex::encode(pred)));
                    }
                }
            }
            OperationState::Done => return Err(TimelockError::AlreadyDone),
            OperationState::Cancelled => return Err(TimelockError::Cancelled),
            OperationState::Ready => {}
        }

        let calls = self.operations.get(&id).unwrap().calls.clone();
        self.operations.get_mut(&id).unwrap().state = OperationState::Done;
        Ok(calls)
    }

    /// Cancel a pending operation. Only the guardian may cancel (veto).
    pub fn cancel(&mut self, id: [u8; 32], caller: Address) -> Result<(), TimelockError> {
        if caller != self.guardian {
            return Err(TimelockError::NotGuardian);
        }
        let op = self.operations.get_mut(&id)
            .ok_or_else(|| TimelockError::NotFound(hex::encode(id)))?;
        match op.state {
            OperationState::Pending { .. } => {
                op.state = OperationState::Cancelled;
                Ok(())
            }
            OperationState::Done => Err(TimelockError::AlreadyDone),
            OperationState::Cancelled => Err(TimelockError::Cancelled),
            OperationState::Ready => {
                op.state = OperationState::Cancelled;
                Ok(())
            }
        }
    }

    /// Update the timelock delay (admin-only).
    pub fn update_delay(&mut self, caller: Address, new_delay: u64) -> Result<(), TimelockError> {
        if caller != self.admin {
            // GOV-V1-02 fix (2026-05-16): use NotAdmin — this check guards the
            // admin role, not the guardian role.  Using NotGuardian was a
            // misleading diagnostic that made callers think they needed the veto
            // guardian address when they actually needed the admin address.
            return Err(TimelockError::NotAdmin);
        }
        if new_delay < MIN_DELAY_SECS || new_delay > MAX_DELAY_SECS {
            return Err(TimelockError::InvalidDelay {
                delay: new_delay,
                min: MIN_DELAY_SECS,
                max: MAX_DELAY_SECS,
            });
        }
        self.delay = new_delay;
        Ok(())
    }

    /// Get the current state of an operation.
    pub fn state(&self, id: &[u8; 32]) -> Option<&OperationState> {
        self.operations.get(id).map(|op| &op.state)
    }

    /// Check if an operation is ready to execute (eta reached, not done/cancelled).
    pub fn is_ready(&self, id: &[u8; 32], now: u64) -> bool {
        self.operations.get(id).map(|op| {
            matches!(&op.state, OperationState::Pending { eta } if now >= *eta)
        }).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(v: u8) -> Address { Address([v; 20]) }

    #[test]
    fn schedule_and_execute() {
        let mut tl = TimelockController::new(MIN_DELAY_SECS, addr(1), addr(0)).unwrap();
        let calls = vec![Call { target: addr(9), calldata: vec![0xab, 0xcd], value: 0 }];
        let salt = [1u8; 32];
        let id = tl.schedule(addr(2), calls, None, salt, 0).unwrap();
        assert!(!tl.is_ready(&id, 0));
        assert!(tl.is_ready(&id, MIN_DELAY_SECS));
        let result = tl.execute(id, MIN_DELAY_SECS).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(tl.state(&id), Some(&OperationState::Done));
    }

    #[test]
    fn not_ready_before_eta() {
        let mut tl = TimelockController::new(MIN_DELAY_SECS, addr(1), addr(0)).unwrap();
        let id = tl.schedule(addr(2), vec![], None, [2u8; 32], 1000).unwrap();
        let err = tl.execute(id, 1000 + MIN_DELAY_SECS - 1).unwrap_err();
        assert!(matches!(err, TimelockError::NotReady { .. }));
    }

    #[test]
    fn guardian_veto_cancels_operation() {
        let mut tl = TimelockController::new(MIN_DELAY_SECS, addr(1), addr(0)).unwrap();
        let id = tl.schedule(addr(2), vec![], None, [3u8; 32], 0).unwrap();
        tl.cancel(id, addr(1)).unwrap();
        assert_eq!(tl.state(&id), Some(&OperationState::Cancelled));
        let err = tl.execute(id, MIN_DELAY_SECS + 1).unwrap_err();
        assert!(matches!(err, TimelockError::Cancelled));
    }

    #[test]
    fn non_guardian_cannot_cancel() {
        let mut tl = TimelockController::new(MIN_DELAY_SECS, addr(1), addr(0)).unwrap();
        let id = tl.schedule(addr(2), vec![], None, [4u8; 32], 0).unwrap();
        assert!(matches!(tl.cancel(id, addr(99)), Err(TimelockError::NotGuardian)));
    }

    #[test]
    fn predecessor_must_be_done_first() {
        let mut tl = TimelockController::new(MIN_DELAY_SECS, addr(1), addr(0)).unwrap();
        let pred_id = tl.schedule(addr(2), vec![], None, [5u8; 32], 0).unwrap();
        let dep_id = tl.schedule(addr(2), vec![], Some(pred_id), [6u8; 32], 0).unwrap();
        let err = tl.execute(dep_id, MIN_DELAY_SECS + 1).unwrap_err();
        assert!(matches!(err, TimelockError::PredecessorPending(_)));
        tl.execute(pred_id, MIN_DELAY_SECS + 1).unwrap();
        tl.execute(dep_id, MIN_DELAY_SECS + 1).unwrap();
    }

    #[test]
    fn invalid_delay_rejected() {
        assert!(TimelockController::new(1, addr(1), addr(0)).is_err());
        assert!(TimelockController::new(MAX_DELAY_SECS + 1, addr(1), addr(0)).is_err());
    }

    #[test]
    fn duplicate_schedule_rejected() {
        let mut tl = TimelockController::new(MIN_DELAY_SECS, addr(1), addr(0)).unwrap();
        tl.schedule(addr(2), vec![], None, [7u8; 32], 0).unwrap();
        let err = tl.schedule(addr(2), vec![], None, [7u8; 32], 0).unwrap_err();
        assert!(matches!(err, TimelockError::AlreadyQueued(_)));
    }
}
