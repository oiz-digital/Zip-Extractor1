//! ZBX Token Billing for AI Inference.
//!
//! Every inference call via 0xCA costs ZBX tokens (in addition to gas).
//! The ZBX token payment is split between:
//!   - Model publisher:      60% (incentivizes model development)
//!   - ZBX DAO treasury:     25% (funds protocol development)
//!   - Validator reward pool: 15% (compensates compute work)
//!
//! Token amounts are computed in wei (10^18 scale).
//!
//! Security:
//! - All payments are atomic: inference only runs if payment succeeds
//! - Payment amounts use integer arithmetic only (no rounding errors)
//! - Minimum payment enforced to prevent spam attacks

use crate::error::RegistryError;
use zbx_ai_precompile::ModelId;
use serde::{Serialize, Deserialize};
use std::collections::HashMap;

/// Minimum fee per inference call: 0.001 ZBX in wei.
pub const MIN_INFERENCE_FEE_WEI: u128 = 1_000_000_000_000_000u128; // 0.001 ZBX

/// Publisher share: 60% (basis points).
pub const PUBLISHER_SHARE_BPS: u32 = 6_000;
/// DAO treasury share: 25%.
pub const TREASURY_SHARE_BPS: u32 = 2_500;
/// Validator pool share: 15%.
pub const VALIDATOR_SHARE_BPS: u32 = 1_500;

/// Fee schedule for each model (in wei per inference).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeeSchedule {
    /// Base fee per inference call (wei).
    pub base_fee_wei: u128,
    /// Per-byte input surcharge (wei per byte).
    pub per_byte_wei: u128,
    /// Minimum fee (always charged).
    pub min_fee_wei:  u128,
}

impl FeeSchedule {
    pub fn default_for_model(id: ModelId) -> Self {
        use zbx_ai_precompile::ModelId::*;
        let base = match id {
            NftTagger          => 5_000_000_000_000_000u128, // 0.005 ZBX
            PricePrediction
            | MarketMaker      => 3_000_000_000_000_000,     // 0.003 ZBX
            RiskScorer
            | ZusdRiskModel
            | FraudDetector
            | OracleAnomalyGuard => 2_000_000_000_000_000,   // 0.002 ZBX
            _                  => 1_000_000_000_000_000,     // 0.001 ZBX (default)
        };
        Self {
            base_fee_wei: base,
            per_byte_wei: 100_000_000_000u128, // 0.0000001 ZBX per byte
            min_fee_wei:  MIN_INFERENCE_FEE_WEI,
        }
    }

    pub fn compute_fee(&self, input_len: usize) -> u128 {
        let total = self.base_fee_wei
            + self.per_byte_wei * input_len as u128;
        total.max(self.min_fee_wei)
    }
}

/// Split of a single fee payment.
#[derive(Debug, Clone)]
pub struct FeeSplit {
    pub total_wei:     u128,
    pub publisher_wei: u128,
    pub treasury_wei:  u128,
    pub validator_wei: u128,
}

impl FeeSplit {
    pub fn compute(total: u128) -> Self {
        let publisher = (total * PUBLISHER_SHARE_BPS as u128) / 10_000;
        let treasury  = (total * TREASURY_SHARE_BPS as u128)  / 10_000;
        // Validator gets remainder (avoids rounding drift)
        let validator = total - publisher - treasury;
        Self {
            total_wei:     total,
            publisher_wei: publisher,
            treasury_wei:  treasury,
            validator_wei: validator,
        }
    }
}

/// Billing record for a single inference call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceBilling {
    pub model_id:    ModelId,
    pub caller:      [u8; 20],
    pub fee_wei:     u128,
    pub block:       u64,
    pub tx_index:    u32,
    pub input_len:   u32,
}

/// Account balance tracker (in-memory; persisted to state in production).
#[derive(Debug, Default, Clone)]
pub struct AccountBalance {
    /// ZBX token balance in wei.
    pub balance_wei: u128,
    /// Total spend on AI inference (wei).
    pub total_spent: u128,
    /// Total inference calls made.
    pub call_count:  u64,
}

impl AccountBalance {
    pub fn credit(&mut self, amount: u128) {
        self.balance_wei = self.balance_wei.saturating_add(amount);
    }

    pub fn debit(&mut self, amount: u128) -> Result<(), RegistryError> {
        if self.balance_wei < amount {
            return Err(RegistryError::InsufficientBalance {
                have: self.balance_wei,
                need: amount,
            });
        }
        self.balance_wei -= amount;
        self.total_spent  = self.total_spent.saturating_add(amount);
        self.call_count  += 1;
        Ok(())
    }
}

/// The billing system — tracks balances and processes inference payments.
pub struct BillingSystem {
    /// fee schedules per model
    fee_schedules:    HashMap<ModelId, FeeSchedule>,
    /// account balances
    balances:         HashMap<[u8; 20], AccountBalance>,
    /// publisher earnings
    publisher_ledger: HashMap<[u8; 20], u128>,
    /// DAO treasury accumulated
    treasury_wei:     u128,
    /// Validator pool accumulated
    validator_pool_wei: u128,
    /// All billing records (capped at 10_000)
    billing_log:      Vec<InferenceBilling>,
}

impl BillingSystem {
    pub fn new() -> Self {
        Self {
            fee_schedules:      HashMap::new(),
            balances:           HashMap::new(),
            publisher_ledger:   HashMap::new(),
            treasury_wei:       0,
            validator_pool_wei: 0,
            billing_log:        Vec::new(),
        }
    }

    pub fn set_fee_schedule(&mut self, id: ModelId, sched: FeeSchedule) {
        self.fee_schedules.insert(id, sched);
    }

    pub fn get_fee_schedule(&self, id: ModelId) -> FeeSchedule {
        self.fee_schedules
            .get(&id)
            .cloned()
            .unwrap_or_else(|| FeeSchedule::default_for_model(id))
    }

    /// Credit ZBX to an account.
    pub fn deposit(&mut self, account: [u8; 20], amount_wei: u128) {
        self.balances.entry(account).or_default().credit(amount_wei);
    }

    /// Process inference payment: deduct from caller, split to parties.
    pub fn charge(
        &mut self,
        caller:      [u8; 20],
        publisher:   [u8; 20],
        model_id:    ModelId,
        input_len:   usize,
        block:       u64,
        tx_index:    u32,
    ) -> Result<FeeSplit, RegistryError> {
        let sched    = self.get_fee_schedule(model_id);
        let fee      = sched.compute_fee(input_len);
        let split    = FeeSplit::compute(fee);

        // Deduct from caller
        self.balances.entry(caller).or_default().debit(fee)?;

        // Credit publisher
        *self.publisher_ledger.entry(publisher).or_default()
            += split.publisher_wei;

        // Credit treasury
        self.treasury_wei      += split.treasury_wei;
        self.validator_pool_wei += split.validator_wei;

        // Log billing record (cap at 10_000)
        if self.billing_log.len() < 10_000 {
            self.billing_log.push(InferenceBilling {
                model_id,
                caller,
                fee_wei:   fee,
                block,
                tx_index,
                input_len: input_len as u32,
            });
        }

        tracing::debug!(
            caller   = ?caller,
            model    = ?model_id,
            fee_wei  = fee,
            block,
            "AI inference charged"
        );

        Ok(split)
    }

    pub fn balance_of(&self, account: &[u8; 20]) -> u128 {
        self.balances.get(account).map(|b| b.balance_wei).unwrap_or(0)
    }

    pub fn publisher_earnings(&self, publisher: &[u8; 20]) -> u128 {
        self.publisher_ledger.get(publisher).copied().unwrap_or(0)
    }

    pub fn treasury_balance(&self) -> u128 { self.treasury_wei }
    pub fn validator_pool(&self) -> u128 { self.validator_pool_wei }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fee_split_sums_to_total() {
        let total = 1_000_000_000_000_000u128;
        let split = FeeSplit::compute(total);
        assert_eq!(split.publisher_wei + split.treasury_wei + split.validator_wei, total);
    }

    #[test]
    fn publisher_gets_60_pct() {
        let total = 10_000_000_000_000_000u128;
        let split = FeeSplit::compute(total);
        assert_eq!(split.publisher_wei, 6_000_000_000_000_000);
    }

    #[test]
    fn insufficient_balance_rejected() {
        let mut sys = BillingSystem::new();
        let caller    = [0x01u8; 20];
        let publisher = [0x02u8; 20];
        // No deposit → insufficient
        let err = sys.charge(caller, publisher, ModelId::SpamClassifier, 32, 1, 0).unwrap_err();
        assert!(matches!(err, RegistryError::InsufficientBalance { .. }));
    }

    #[test]
    fn charge_succeeds_with_balance() {
        let mut sys = BillingSystem::new();
        let caller    = [0x01u8; 20];
        let publisher = [0x02u8; 20];
        sys.deposit(caller, 100_000_000_000_000_000u128); // 0.1 ZBX
        let split = sys.charge(caller, publisher, ModelId::SpamClassifier, 32, 1, 0).unwrap();
        assert!(split.total_wei >= MIN_INFERENCE_FEE_WEI);
        assert!(sys.publisher_earnings(&publisher) > 0);
        assert!(sys.treasury_balance() > 0);
    }

    #[test]
    fn per_byte_surcharge_increases_fee() {
        let sched = FeeSchedule::default_for_model(ModelId::NftTagger);
        let fee_small = sched.compute_fee(10);
        let fee_large = sched.compute_fee(500);
        assert!(fee_large > fee_small);
    }
}
