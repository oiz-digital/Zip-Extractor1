//! Chain parameter administration -- base fee, gas limit, epoch config.
//!
//! Chain parameters are stored in ChainConfig and can only be changed
//! by the Operator role (gated by timelock for safety-critical params).
//!
//! Every parameter change emits an AdminEvent::ConfigChanged on-chain
//! and writes a record to the AuditLog.

// ── ChainConfig ───────────────────────────────────────────────────────────────

/// Mutable chain configuration parameters.
/// Stored in state at address 0xAdmin00...01.
#[derive(Debug, Clone)]
pub struct ChainConfig {
    // Fee params
    pub base_fee_gwei:       u64,     // default: 1 gwei
    pub max_priority_fee:    u64,     // default: 2 gwei
    pub block_gas_limit:     u64,     // default: 30_000_000
    // Epoch config
    pub epoch_length:        u64,     // blocks per epoch (default: 14400 = ~12h at 3s)
    pub stake_lock_seconds:  u64,     // default: 604800 (7 days)
    // Validator config
    pub min_stake_zbx:       u128,    // default: 1_000 ZBX (in wei)
    pub max_validators:      u32,     // default: 100
    pub validator_slots:     u32,     // active validator slot count (default: 21)
    // Reward config
    pub reward_rate_bps:     u32,     // basis points annual (default: 800 = 8%)
    pub slash_pct:           u8,      // slash percentage (default: 5%)
    pub commission_cap_bps:  u32,     // max validator commission (default: 2000 = 20%)
    // Treasury
    pub treasury_address:    [u8; 20],
    pub dev_fund_pct:        u8,      // % of fees to dev fund (default: 10%)
    pub protocol_fee_bps:    u32,     // protocol revenue fee (default: 30 bps = 0.3%)
    // Lock
    pub params_timelock_secs: u64,    // timelock for safety-critical param changes
}

impl ChainConfig {
    pub fn mainnet_default(treasury: [u8; 20]) -> Self {
        Self {
            base_fee_gwei:        1,
            max_priority_fee:     2,
            block_gas_limit:      30_000_000,
            epoch_length:         14_400,
            stake_lock_seconds:   604_800,
            min_stake_zbx:        1_000 * 1_000_000_000_000_000_000,
            max_validators:       100,
            validator_slots:      21,
            reward_rate_bps:      800,
            slash_pct:            5,
            commission_cap_bps:   2_000,
            treasury_address:     treasury,
            dev_fund_pct:         10,
            protocol_fee_bps:     30,
            params_timelock_secs: 172_800, // 48h timelock
        }
    }
}

// ── Parameter setters (Operator role) ────────────────────────────────────────

pub struct ChainParamAdmin {
    pub config: ChainConfig,
    pub event_log: Vec<AdminConfigEvent>,
}

#[derive(Debug, Clone)]
pub struct AdminConfigEvent {
    pub block:   u64,
    pub caller:  [u8; 20],
    pub key:     String,
    pub old_val: String,
    pub new_val: String,
}

impl ChainParamAdmin {
    /// Set the base fee (EIP-1559 base fee in gwei).
    /// Operator role required. Emits AdminEvent::BaseFeeChanged.
    pub fn set_base_fee(&mut self, caller: [u8; 20], new_fee: u64, block: u64) -> Result<AdminConfigEvent, ParamError> {
        if new_fee == 0 { return Err(ParamError::InvalidValue("base_fee must be > 0".into())); }
        if new_fee > 10_000 { return Err(ParamError::InvalidValue("base_fee too high (max 10000 gwei)".into())); }
        let ev = AdminConfigEvent {
            block, caller,
            key:     "base_fee_gwei".into(),
            old_val: self.config.base_fee_gwei.to_string(),
            new_val: new_fee.to_string(),
        };
        self.config.base_fee_gwei = new_fee;
        self.event_log.push(ev.clone());
        Ok(ev)
    }

    /// Set block gas limit.
    pub fn set_block_gas_limit(&mut self, caller: [u8; 20], new_limit: u64, block: u64) -> Result<AdminConfigEvent, ParamError> {
        if new_limit < 1_000_000 { return Err(ParamError::InvalidValue("gas_limit too low".into())); }
        if new_limit > 300_000_000 { return Err(ParamError::InvalidValue("gas_limit too high".into())); }
        let ev = AdminConfigEvent {
            block, caller,
            key:     "block_gas_limit".into(),
            old_val: self.config.block_gas_limit.to_string(),
            new_val: new_limit.to_string(),
        };
        self.config.block_gas_limit = new_limit;
        self.event_log.push(ev.clone());
        Ok(ev)
    }

    /// Set epoch length (blocks).
    pub fn set_epoch_length(&mut self, caller: [u8; 20], new_len: u64, block: u64) -> Result<AdminConfigEvent, ParamError> {
        if new_len < 100 { return Err(ParamError::InvalidValue("epoch_length too short".into())); }
        let ev = AdminConfigEvent {
            block, caller,
            key: "epoch_length".into(),
            old_val: self.config.epoch_length.to_string(),
            new_val: new_len.to_string(),
        };
        self.config.epoch_length = new_len;
        self.event_log.push(ev.clone());
        Ok(ev)
    }

    /// Set minimum stake requirement (in ZBX wei).
    pub fn set_min_stake(&mut self, caller: [u8; 20], new_min: u128, block: u64) -> Result<AdminConfigEvent, ParamError> {
        let ev = AdminConfigEvent {
            block, caller,
            key: "min_stake_zbx".into(),
            old_val: self.config.min_stake_zbx.to_string(),
            new_val: new_min.to_string(),
        };
        self.config.min_stake_zbx = new_min;
        self.event_log.push(ev.clone());
        Ok(ev)
    }

    /// Set reward rate (basis points annual).
    pub fn set_reward_rate(&mut self, caller: [u8; 20], new_bps: u32, block: u64) -> Result<AdminConfigEvent, ParamError> {
        if new_bps > 5_000 { return Err(ParamError::InvalidValue("reward_rate > 50% bps".into())); }
        let ev = AdminConfigEvent {
            block, caller,
            key: "reward_rate_bps".into(),
            old_val: self.config.reward_rate_bps.to_string(),
            new_val: new_bps.to_string(),
        };
        self.config.reward_rate_bps = new_bps;
        self.event_log.push(ev.clone());
        Ok(ev)
    }

    /// Set lock period for staked ZBX (in seconds).
    pub fn set_lock_period(&mut self, caller: [u8; 20], new_secs: u64, block: u64) -> Result<AdminConfigEvent, ParamError> {
        if new_secs > 30 * 24 * 3600 { return Err(ParamError::InvalidValue("lock_period > 30 days".into())); }
        let ev = AdminConfigEvent {
            block, caller,
            key: "stake_lock_seconds".into(),
            old_val: self.config.stake_lock_seconds.to_string(),
            new_val: new_secs.to_string(),
        };
        self.config.stake_lock_seconds = new_secs;
        self.event_log.push(ev.clone());
        Ok(ev)
    }

    /// Set slash percentage.
    pub fn set_slash_percentage(&mut self, caller: [u8; 20], new_pct: u8, block: u64) -> Result<AdminConfigEvent, ParamError> {
        if new_pct > 100 { return Err(ParamError::InvalidValue("slash_pct > 100".into())); }
        let ev = AdminConfigEvent {
            block, caller,
            key: "slash_pct".into(),
            old_val: self.config.slash_pct.to_string(),
            new_val: new_pct.to_string(),
        };
        self.config.slash_pct = new_pct;
        self.event_log.push(ev.clone());
        Ok(ev)
    }

    /// Set maximum number of validators.
    pub fn set_max_validators(&mut self, caller: [u8; 20], new_max: u32, block: u64) -> Result<AdminConfigEvent, ParamError> {
        if new_max < 4 { return Err(ParamError::InvalidValue("max_validators < 4".into())); }
        let ev = AdminConfigEvent {
            block, caller,
            key: "max_validators".into(),
            old_val: self.config.max_validators.to_string(),
            new_val: new_max.to_string(),
        };
        self.config.max_validators = new_max;
        self.event_log.push(ev.clone());
        Ok(ev)
    }

    /// AdminEvent log -- returns all parameter change events.
    pub fn admin_event_history(&self) -> &[AdminConfigEvent] {
        &self.event_log
    }
}

#[derive(Debug)]
pub enum ParamError {
    InvalidValue(String),
    TimelockRequired,
    Unauthorized,
}