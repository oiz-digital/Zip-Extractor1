//! Epoch processing — ZBX PoS epoch transitions.
//! Handles activation/exit queues, rewards, randao mixing, registry updates.

use std::collections::VecDeque;
use crate::types::{Epoch, Slot, ValidatorIndex};
use crate::consensus::{SLOTS_PER_EPOCH, SECONDS_PER_SLOT};

/// Epoch-boundary processing result
#[derive(Debug, Clone, Default)]
pub struct EpochProcessingResult {
    pub epoch: Epoch,
    pub activated: Vec<ValidatorIndex>,
    pub exited: Vec<ValidatorIndex>,
    pub slashed: Vec<ValidatorIndex>,
    pub total_rewards: u64,
    pub total_penalties: u64,
    pub new_randao_mix: [u8; 32],
    pub new_justified_epoch: Option<Epoch>,
    pub new_finalized_epoch: Option<Epoch>,
}

/// Activation queue entry
#[derive(Debug, Clone)]
pub struct ActivationQueueEntry {
    pub validator_index: ValidatorIndex,
    pub eligibility_epoch: Epoch,
    pub stake: u64,
}

/// Exit queue entry
#[derive(Debug, Clone)]
pub struct ExitQueueEntry {
    pub validator_index: ValidatorIndex,
    pub exit_epoch: Epoch,
    pub withdrawable_epoch: Epoch,
}

/// Epoch processor
pub struct EpochProcessor {
    /// Maximum churn per epoch (validators activated/exited per epoch)
    pub max_churn: usize,
    /// Activation queue
    pub activation_queue: VecDeque<ActivationQueueEntry>,
    /// Exit queue
    pub exit_queue: VecDeque<ExitQueueEntry>,
    /// Historical epoch data (last 64 epochs)
    pub history: VecDeque<EpochSnapshot>,
    /// RANDAO mixes (last 65536 slots)
    pub randao_mixes: Vec<[u8; 32]>,
    /// Historical balances (for slashing calculations)
    pub historical_balances: VecDeque<Vec<u64>>,
}

/// Snapshot of epoch state
#[derive(Debug, Clone)]
pub struct EpochSnapshot {
    pub epoch: Epoch,
    pub active_validators: usize,
    pub total_staked: u64,
    pub justified: bool,
    pub finalized: bool,
    pub participation_rate: f64, // 0.0-1.0
    pub randao_mix: [u8; 32],
}

impl EpochProcessor {
    pub fn new(randao_mixes: Vec<[u8; 32]>) -> Self {
        Self {
            max_churn: 4,
            activation_queue: VecDeque::new(),
            exit_queue: VecDeque::new(),
            history: VecDeque::with_capacity(64),
            randao_mixes,
            historical_balances: VecDeque::with_capacity(8),
        }
    }

    /// Process epoch transition
    pub fn process(&mut self, epoch: Epoch, state: &mut dyn EpochState) -> EpochProcessingResult {
        let mut result = EpochProcessingResult { epoch, ..Default::default() };

        // 1. Compute attesting balance for justification
        let total_balance = state.total_active_balance(epoch);
        let prev_attesting = state.attesting_balance(epoch.saturating_sub(1));
        let curr_attesting = state.attesting_balance(epoch);

        // 2. Justification/Finalization
        if prev_attesting * 3 >= total_balance * 2 {
            result.new_justified_epoch = Some(epoch - 1);
        }
        if curr_attesting * 3 >= total_balance * 2 {
            result.new_justified_epoch = Some(epoch);
        }
        if let Some(j) = result.new_justified_epoch {
            if j + 2 <= epoch {
                result.new_finalized_epoch = Some(j);
            }
        }

        // 3. Process activation queue
        let mut churn = 0;
        while churn < self.max_churn {
            if let Some(entry) = self.activation_queue.front() {
                if entry.eligibility_epoch <= epoch {
                    let e = self.activation_queue.pop_front().unwrap();
                    state.activate_validator(e.validator_index, epoch + 1);
                    result.activated.push(e.validator_index);
                    churn += 1;
                    continue;
                }
            }
            break;
        }

        // 4. Process exit queue
        let mut exit_churn = 0;
        while exit_churn < self.max_churn {
            if let Some(entry) = self.exit_queue.front() {
                if entry.exit_epoch <= epoch {
                    let e = self.exit_queue.pop_front().unwrap();
                    state.exit_validator(e.validator_index);
                    result.exited.push(e.validator_index);
                    exit_churn += 1;
                    continue;
                }
            }
            break;
        }

        // 5. Compute rewards & penalties
        let (rewards, penalties) = self.compute_rewards_penalties(epoch, state, total_balance, prev_attesting);
        result.total_rewards = rewards;
        result.total_penalties = penalties;

        // 6. Update RANDAO mix
        result.new_randao_mix = self.compute_randao_mix(epoch, state.randao_reveal(epoch));

        // 7. Update historical balances
        self.historical_balances.push_back(state.validator_balances(epoch));
        if self.historical_balances.len() > 8 { self.historical_balances.pop_front(); }

        // 8. Record epoch snapshot
        let participation_rate = if total_balance > 0 {
            curr_attesting as f64 / total_balance as f64
        } else { 0.0 };
        self.history.push_back(EpochSnapshot {
            epoch,
            active_validators: state.active_validator_count(epoch),
            total_staked: total_balance,
            justified: result.new_justified_epoch.is_some(),
            finalized: result.new_finalized_epoch.is_some(),
            participation_rate,
            randao_mix: result.new_randao_mix,
        });
        if self.history.len() > 64 { self.history.pop_front(); }

        tracing::info!(
            epoch,
            activated = result.activated.len(),
            exited = result.exited.len(),
            rewards = result.total_rewards,
            penalties = result.total_penalties,
            participation = format!("{:.1}%", participation_rate * 100.0),
            "Epoch processed"
        );

        result
    }

    /// Compute per-validator rewards and penalties
    fn compute_rewards_penalties(
        &self,
        epoch: Epoch,
        state: &dyn EpochState,
        total_balance: u64,
        attesting_balance: u64,
    ) -> (u64, u64) {
        let base_reward_factor = 64u64;
        let active_count = state.active_validator_count(epoch);
        if active_count == 0 { return (0, 0); }

        let sqrt_total = integer_sqrt(total_balance);
        let mut total_rewards = 0u64;
        let mut total_penalties = 0u64;

        for idx in 0..active_count as ValidatorIndex {
            let eff_bal = state.effective_balance(idx);
            let base_reward = eff_bal * base_reward_factor / sqrt_total / 4;

            if state.did_attest(idx, epoch) {
                // Source + target + head rewards (3/4 of base_reward * participation_fraction)
                let source = base_reward * attesting_balance / total_balance;
                total_rewards += source * 3;
                // Inclusion delay reward
                total_rewards += base_reward / 4;
            } else {
                // Penalty for missing
                total_penalties += base_reward * 3;
                // Inactivity leak
                if self.is_inactivity_leak(epoch) {
                    total_penalties += eff_bal / 65536 * 4;
                }
            }
        }
        (total_rewards, total_penalties)
    }

    /// Compute next RANDAO mix (XOR of existing mix with new reveal hash)
    fn compute_randao_mix(&self, epoch: Epoch, reveal: [u8; 32]) -> [u8; 32] {
        use sha2::{Sha256, Digest};
        let idx = (epoch as usize) % self.randao_mixes.len().max(1);
        let existing = self.randao_mixes.get(idx).copied().unwrap_or([0u8; 32]);
        let mut h = Sha256::new();
        h.update(reveal);
        let reveal_hash: [u8; 32] = h.finalize().into();
        // XOR
        let mut mix = [0u8; 32];
        for i in 0..32 { mix[i] = existing[i] ^ reveal_hash[i]; }
        mix
    }

    fn is_inactivity_leak(&self, epoch: Epoch) -> bool {
        if self.history.len() < 4 { return false; }
        self.history.iter().rev().take(4).all(|s| !s.finalized)
    }

    /// Enqueue validator for activation
    pub fn enqueue_activation(&mut self, validator_index: ValidatorIndex, eligibility_epoch: Epoch, stake: u64) {
        self.activation_queue.push_back(ActivationQueueEntry { validator_index, eligibility_epoch, stake });
        // Sort by eligibility epoch then stake (descending)
        let mut v: Vec<_> = self.activation_queue.drain(..).collect();
        v.sort_by(|a, b| a.eligibility_epoch.cmp(&b.eligibility_epoch).then(b.stake.cmp(&a.stake)));
        self.activation_queue = v.into();
    }

    /// Enqueue validator for exit
    pub fn enqueue_exit(&mut self, validator_index: ValidatorIndex, exit_epoch: Epoch) {
        let withdrawable_epoch = exit_epoch + 256;
        self.exit_queue.push_back(ExitQueueEntry { validator_index, exit_epoch, withdrawable_epoch });
    }
}

/// Trait for accessing epoch state data
pub trait EpochState {
    fn total_active_balance(&self, epoch: Epoch) -> u64;
    fn attesting_balance(&self, epoch: Epoch) -> u64;
    fn active_validator_count(&self, epoch: Epoch) -> usize;
    fn effective_balance(&self, idx: ValidatorIndex) -> u64;
    fn did_attest(&self, idx: ValidatorIndex, epoch: Epoch) -> bool;
    fn randao_reveal(&self, epoch: Epoch) -> [u8; 32];
    fn activate_validator(&mut self, idx: ValidatorIndex, activation_epoch: Epoch);
    fn exit_validator(&mut self, idx: ValidatorIndex);
    fn validator_balances(&self, epoch: Epoch) -> Vec<u64>;
}

fn integer_sqrt(n: u64) -> u64 {
    if n == 0 { return 0; }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x { x = y; y = (x + n / x) / 2; }
    x
}