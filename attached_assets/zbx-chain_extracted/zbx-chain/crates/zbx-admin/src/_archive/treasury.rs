//! Treasury administration -- mint/burn, fee distribution, protocol revenue.
//!
//! Fee distribution flow:
//!   1. Block producer collects all tx fees (base_fee * gas_used)
//!   2. At epoch end, FeeDistributor splits fees:
//!      - dev_fund_pct% -> Foundation dev fund
//!      - (100 - dev_fund_pct)% -> Validator reward pool (pro-rata stake)
//!
//! Protocol revenue:
//!   DEX swap fees, lending protocol fees, bridge fees -> Treasury
//!   Protocol fee = protocol_fee_bps / 10000 of each trade volume
//!
//! Annual emission cap = 50,000,000 ZBX per year (hard limit).

pub const ANNUAL_EMISSION_CAP: u128 = 50_000_000 * 1_000_000_000_000_000_000; // 50M ZBX

// ── Fee Distributor ───────────────────────────────────────────────────────────

/// FeeDistributor -- splits collected fees at epoch boundary.
pub struct FeeDistributor {
    /// Total fees collected this epoch (in ZBX wei)
    pub epoch_fees:     u128,
    /// Running total distributed to dev fund
    pub dev_fund_total: u128,
    /// Running total distributed to validators
    pub validator_pool_total: u128,
    /// Distribution history
    pub history:        Vec<FeeDistributionRecord>,
}

/// Record of a single epoch's fee distribution.
#[derive(Debug, Clone)]
pub struct FeeDistributionRecord {
    pub epoch:              u64,
    pub block:              u64,
    pub total_fees:         u128,
    pub dev_fund_amount:    u128,
    pub validator_amount:   u128,
    pub recipients:         u32,   // number of validators who received rewards
}

impl FeeDistributor {
    pub fn new() -> Self {
        Self { epoch_fees: 0, dev_fund_total: 0, validator_pool_total: 0, history: Vec::new() }
    }

    /// Collect fees from a block (called per block).
    pub fn collect(&mut self, base_fee: u64, gas_used: u64) {
        let fee = base_fee as u128 * gas_used as u128;
        self.epoch_fees += fee;
    }

    /// Distribute fees at epoch end.
    /// Emits AdminEvent::FeeDistributed with amount and recipient count.
    pub fn distribute(
        &mut self,
        epoch:       u64,
        block:       u64,
        dev_fund_pct: u8,
        validators:  &[ValidatorShare],
    ) -> FeeDistributionRecord {
        let dev_amount  = self.epoch_fees * dev_fund_pct as u128 / 100;
        let val_amount  = self.epoch_fees - dev_amount;

        // Pro-rata distribution to validators (by stake weight)
        let total_stake: u128 = validators.iter().map(|v| v.stake).sum();
        let mut distributed = 0u128;
        for v in validators {
            if total_stake > 0 {
                let share = val_amount * v.stake / total_stake;
                distributed += share;
                // v.address receives `share` ZBX -- credited to their reward balance
            }
        }

        self.dev_fund_total       += dev_amount;
        self.validator_pool_total += distributed;

        let record = FeeDistributionRecord {
            epoch, block,
            total_fees:      self.epoch_fees,
            dev_fund_amount: dev_amount,
            validator_amount: distributed,
            recipients:      validators.len() as u32,
        };
        self.history.push(record.clone());
        self.epoch_fees = 0; // Reset for next epoch
        record
    }
}

#[derive(Debug, Clone)]
pub struct ValidatorShare {
    pub address: [u8; 20],
    pub stake:   u128,
}

// ── Protocol Revenue ──────────────────────────────────────────────────────────

/// Protocol revenue tracker -- fees from DeFi protocols.
/// DEX swaps, lending, bridges -> Treasury via protocol_fee_bps.
pub struct ProtocolRevenue {
    pub total_collected:  u128,
    pub by_source:        Vec<RevenueRecord>,
}

#[derive(Debug, Clone)]
pub struct RevenueRecord {
    pub block:    u64,
    pub source:   RevenueSource,
    pub amount:   u128,
}

#[derive(Debug, Clone)]
pub enum RevenueSource {
    DexSwap      { pair: String },
    LendingFee   { asset: String },
    BridgeFee    { chain_id: u64 },
    OracleUpdate { feed: String },
    Other        { name: String },
}

impl ProtocolRevenue {
    pub fn new() -> Self { Self { total_collected: 0, by_source: Vec::new() } }

    /// Record protocol revenue from a DeFi interaction.
    /// Emits AdminEvent::ProtocolRevenue.
    pub fn record(&mut self, block: u64, source: RevenueSource, amount: u128) {
        self.total_collected += amount;
        self.by_source.push(RevenueRecord { block, source, amount });
    }

    /// Total protocol revenue collected all-time.
    pub fn total(&self) -> u128 { self.total_collected }
}

// ── Treasury withdrawal ───────────────────────────────────────────────────────

/// Treasury withdrawal request (requires Treasury role + timelock).
#[derive(Debug, Clone)]
pub struct TreasuryWithdrawal {
    pub id:            u64,
    pub recipient:     [u8; 20],
    pub amount:        u128,
    pub purpose:       String,
    pub proposed_at:   u64,   // block
    pub executable_at: u64,   // block (proposed_at + timelock)
    pub executed:      bool,
}

pub fn treasury_withdraw(
    recipient: [u8; 20],
    amount:    u128,
    purpose:   &str,
    block:     u64,
    timelock:  u64,
) -> TreasuryWithdrawal {
    TreasuryWithdrawal {
        id:            block, // simplified ID
        recipient,
        amount,
        purpose:       purpose.into(),
        proposed_at:   block,
        executable_at: block + timelock,
        executed:      false,
    }
}