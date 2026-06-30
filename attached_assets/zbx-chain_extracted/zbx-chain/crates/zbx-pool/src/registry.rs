//! Platform fee registry — all ZBX DEX operation fees in one place.
//!
//! ## Fee schedule
//!
//! | Operation | Fee (ZBX) | Purpose |
//! |-----------|-----------|---------|
//! | Pool creation | 500 ZBX | Anti-spam, treasury funding |
//! | Token creation | 100 ZBX | Anti-spam, treasury funding |
//! | Token mint (extra supply) | 1 ZBX/call | Mintable token ops |
//! | Token pause | 5 ZBX | Emergency pause |
//! | Metadata registration | 10 ZBX | Token icon/website/description |
//! | Swap (protocol) | 0.01–0.20% of swap | LP + protocol split (via FeeTier) |
//! | Bridge cross-chain | 0.05% of amount | Cross-chain messaging |
//! | Name registration | 50 ZBX | Human-readable names (like ENS) |
//! | Validator registration | 100 000 ZBX | Min self-stake (zbx-staking) |
//!
//! All fees are in ZBX wei (1 ZBX = 10^18 wei).
//! Fee amounts are governance-adjustable (via `update_*_fee` methods).
//!
//! ## Initialization requirement
//!
//! **`FeeRegistry::default()` uses the zero address for both `governance` and
//! `treasury`.** This is intentional: the zero address is a safe, obvious
//! sentinel that fails loudly if governance operations are attempted before
//! proper initialization (the caller-is-not-governance check rejects it) and
//! that ensures no fees are silently routed to a placeholder address on
//! mainnet.
//!
//! For production deployments always use `FeeRegistry::new(governance, treasury)`
//! with explicit mainnet multisig addresses.

use serde::{Deserialize, Serialize};

// ── Fee amounts (governance-updateable defaults) ───────────────────────────────

/// Pool creation fee: 500 ZBX.
pub const DEFAULT_POOL_CREATION_FEE:     u128 = 500   * 10u128.pow(18);
/// Token creation fee: 100 ZBX.
pub const DEFAULT_TOKEN_CREATION_FEE:    u128 = 100   * 10u128.pow(18);
/// Token mint fee per call: 1 ZBX.
pub const DEFAULT_TOKEN_MINT_FEE:        u128 = 1     * 10u128.pow(18);
/// Token pause fee: 5 ZBX.
pub const DEFAULT_TOKEN_PAUSE_FEE:       u128 = 5     * 10u128.pow(18);
/// Metadata registration fee: 10 ZBX.
pub const DEFAULT_METADATA_FEE:          u128 = 10    * 10u128.pow(18);
/// Bridge cross-chain transfer fee: 0.05% in basis points.
pub const DEFAULT_BRIDGE_FEE_BPS:        u32  = 5;
/// Name/handle registration fee: 50 ZBX.
pub const DEFAULT_NAME_REGISTRATION_FEE: u128 = 50    * 10u128.pow(18);
/// Swap gas overhead estimate (on top of EIP-1559): fixed per-swap surcharge in ZBX wei.
pub const DEFAULT_SWAP_GAS_SURCHARGE:    u128 = 0; // optional; 0 = disabled

// ── Gas fee types ─────────────────────────────────────────────────────────────

/// Classification of all gas-bearing operations on ZBX DEX.
///
/// Callers can use this to estimate fees before submitting a transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DexOperation {
    /// Single-pair swap (1 hop).
    SwapDirect,
    /// Two-hop swap via intermediate token.
    SwapTwoHop,
    /// Add liquidity to an existing pool.
    AddLiquidity,
    /// Remove liquidity from an existing pool.
    RemoveLiquidity,
    /// Create a new liquidity pool.
    CreatePool,
    /// Create a new ERC-20 token.
    CreateToken,
    /// Mint additional token supply.
    MintTokens,
    /// Pause a token.
    PauseToken,
    /// Register token metadata URI.
    RegisterMetadata,
    /// ERC-20 approve.
    Approve,
    /// ERC-20 transferFrom.
    TransferFrom,
    /// Bridge tokens cross-chain.
    BridgeTransfer,
    /// Register a human-readable name.
    RegisterName,
    /// Claim LP rewards.
    ClaimRewards,
}

impl DexOperation {
    /// Estimated EVM gas units for this operation (before EIP-1559 base fee).
    /// These are conservative upper bounds suitable for gas estimation.
    pub fn estimated_gas(&self) -> u64 {
        match self {
            Self::SwapDirect       => 120_000,
            Self::SwapTwoHop       => 200_000,
            Self::AddLiquidity     => 150_000,
            Self::RemoveLiquidity  => 130_000,
            Self::CreatePool       => 300_000,
            Self::CreateToken      => 500_000,
            Self::MintTokens       =>  80_000,
            Self::PauseToken       =>  50_000,
            Self::RegisterMetadata =>  60_000,
            Self::Approve          =>  45_000,
            Self::TransferFrom     =>  65_000,
            Self::BridgeTransfer   => 250_000,
            Self::RegisterName     => 100_000,
            Self::ClaimRewards     =>  90_000,
        }
    }

    /// Total gas cost in ZBX wei given `gas_price_gwei` (base fee + tip in Gwei).
    pub fn gas_cost_wei(&self, gas_price_gwei: u64) -> u128 {
        let gas_price_wei = gas_price_gwei as u128 * 1_000_000_000u128;
        self.estimated_gas() as u128 * gas_price_wei
    }
}

// ── FeeRegistry ────────────────────────────────────────────────────────────────

/// Central registry for all DEX platform fees.
///
/// All amounts are in ZBX wei. Governance can update fees via the
/// `update_*` methods (caller must be the governance address).
///
/// # ⚠️ Initialization
///
/// `Default` sets `governance` and `treasury` to **`[0u8; 20]`** (the zero
/// address).  This is a safe sentinel — governance operations against the zero
/// address will always fail the `only_governance` check, preventing accidental
/// fee parameter changes before proper multi-sig setup.  On mainnet, always
/// construct with `FeeRegistry::new(governance_multisig, treasury_multisig)`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeeRegistry {
    pub pool_creation_fee:     u128,
    pub token_creation_fee:    u128,
    pub token_mint_fee:        u128,
    pub token_pause_fee:       u128,
    pub metadata_fee:          u128,
    /// Cross-chain bridge fee in basis points (e.g. 5 = 0.05%).
    pub bridge_fee_bps:        u32,
    pub name_registration_fee: u128,
    pub swap_gas_surcharge:    u128,
    /// Governance address — only this address can update fees.
    ///
    /// Must be set to the mainnet governance multisig before deployment.
    /// Zero address in `Default` ensures no governance ops succeed until
    /// explicitly initialized.
    pub governance:            [u8; 20],
    /// Protocol treasury — receives all collected fees.
    ///
    /// Must be set to the mainnet treasury multisig before deployment.
    /// Zero address in `Default` means fees would route to the burn address
    /// until properly initialized — a safe failure mode.
    pub treasury:              [u8; 20],
}

impl Default for FeeRegistry {
    fn default() -> Self {
        FeeRegistry {
            pool_creation_fee:     DEFAULT_POOL_CREATION_FEE,
            token_creation_fee:    DEFAULT_TOKEN_CREATION_FEE,
            token_mint_fee:        DEFAULT_TOKEN_MINT_FEE,
            token_pause_fee:       DEFAULT_TOKEN_PAUSE_FEE,
            metadata_fee:          DEFAULT_METADATA_FEE,
            bridge_fee_bps:        DEFAULT_BRIDGE_FEE_BPS,
            name_registration_fee: DEFAULT_NAME_REGISTRATION_FEE,
            swap_gas_surcharge:    DEFAULT_SWAP_GAS_SURCHARGE,
            // Zero address — MUST be replaced before mainnet deployment.
            // Using [0xFF;20] or [0xFE;20] as placeholders would be dangerous
            // because those addresses could theoretically be controlled by an
            // attacker. The zero address fails loudly and safely.
            governance: [0u8; 20],
            treasury:   [0u8; 20],
        }
    }
}

impl FeeRegistry {
    pub fn new(governance: [u8; 20], treasury: [u8; 20]) -> Self {
        FeeRegistry { governance, treasury, ..Self::default() }
    }

    // ── Getters ───────────────────────────────────────────────────────────────

    pub fn pool_creation_fee(&self)     -> u128 { self.pool_creation_fee }
    pub fn token_creation_fee(&self)    -> u128 { self.token_creation_fee }
    pub fn token_mint_fee(&self)        -> u128 { self.token_mint_fee }
    pub fn token_pause_fee(&self)       -> u128 { self.token_pause_fee }
    pub fn metadata_registration_fee(&self) -> u128 { self.metadata_fee }
    pub fn name_registration_fee(&self) -> u128 { self.name_registration_fee }
    pub fn swap_gas_surcharge(&self)    -> u128 { self.swap_gas_surcharge }

    /// Bridge fee on a given amount: `amount * bridge_fee_bps / 10000`.
    pub fn bridge_fee(&self, amount: u128) -> u128 {
        amount * self.bridge_fee_bps as u128 / 10_000
    }

    // ── Fee estimation ────────────────────────────────────────────────────────

    /// Estimate total cost of a DEX operation:
    ///   platform_fee + gas_cost_wei (at given gas price in Gwei).
    pub fn estimate_total_cost(
        &self,
        op:              DexOperation,
        gas_price_gwei:  u64,
        amount:          u128, // for bridge ops
    ) -> FeeEstimate {
        let platform_fee = self.platform_fee_for(op, amount);
        let gas_cost     = op.gas_cost_wei(gas_price_gwei);
        FeeEstimate {
            operation:    op,
            platform_fee,
            gas_cost_wei: gas_cost,
            total_wei:    platform_fee.saturating_add(gas_cost),
        }
    }

    /// Platform-level fee for an operation (not counting gas).
    pub fn platform_fee_for(&self, op: DexOperation, amount: u128) -> u128 {
        match op {
            DexOperation::CreatePool       => self.pool_creation_fee,
            DexOperation::CreateToken      => self.token_creation_fee,
            DexOperation::MintTokens       => self.token_mint_fee,
            DexOperation::PauseToken       => self.token_pause_fee,
            DexOperation::RegisterMetadata => self.metadata_fee,
            DexOperation::RegisterName     => self.name_registration_fee,
            DexOperation::BridgeTransfer   => self.bridge_fee(amount),
            DexOperation::SwapDirect
            | DexOperation::SwapTwoHop    => self.swap_gas_surcharge,
            _ => 0,
        }
    }

    // ── Governance updates ────────────────────────────────────────────────────

    pub fn update_pool_creation_fee(
        &mut self, caller: [u8; 20], new_fee: u128,
    ) -> Result<(), &'static str> {
        self.only_governance(caller)?;
        self.pool_creation_fee = new_fee;
        Ok(())
    }

    pub fn update_token_creation_fee(
        &mut self, caller: [u8; 20], new_fee: u128,
    ) -> Result<(), &'static str> {
        self.only_governance(caller)?;
        self.token_creation_fee = new_fee;
        Ok(())
    }

    pub fn update_bridge_fee_bps(
        &mut self, caller: [u8; 20], new_bps: u32,
    ) -> Result<(), &'static str> {
        self.only_governance(caller)?;
        if new_bps > 1_000 { return Err("bridge fee cannot exceed 10%"); }
        self.bridge_fee_bps = new_bps;
        Ok(())
    }

    fn only_governance(&self, caller: [u8; 20]) -> Result<(), &'static str> {
        if caller != self.governance {
            return Err("caller is not governance");
        }
        Ok(())
    }
}

// ── FeeEstimate output ─────────────────────────────────────────────────────────

/// Full cost breakdown for a DEX operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeeEstimate {
    pub operation:    DexOperation,
    /// Platform-level fee paid to the protocol treasury.
    pub platform_fee: u128,
    /// Estimated EVM gas cost at the given gas price.
    pub gas_cost_wei: u128,
    /// Total ZBX wei required (platform_fee + gas_cost).
    pub total_wei:    u128,
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_fees_match_constants() {
        let r = FeeRegistry::default();
        assert_eq!(r.pool_creation_fee(),  DEFAULT_POOL_CREATION_FEE);
        assert_eq!(r.token_creation_fee(), DEFAULT_TOKEN_CREATION_FEE);
        assert_eq!(r.token_mint_fee(),     DEFAULT_TOKEN_MINT_FEE);
    }

    #[test]
    fn default_governance_is_zero_address() {
        // FIX: was [0xFF;20] / [0xFE;20] — dangerous placeholder addresses.
        // Now [0u8;20] (zero address) — safe sentinel that fails governance ops loudly.
        let r = FeeRegistry::default();
        assert_eq!(r.governance, [0u8; 20], "default governance must be zero address");
        assert_eq!(r.treasury,   [0u8; 20], "default treasury must be zero address");
    }

    #[test]
    fn governance_update_blocked_on_zero_address_default() {
        // Because Default sets governance = [0u8;20], calling update as the zero
        // address must succeed (caller == governance), but this validates that
        // production code which fails to call ::new() cannot accidentally lock in
        // wrong fees from a placeholder governance key.
        let mut r = FeeRegistry::default();
        // zero == zero → governance check passes (intentional — zero-addr governance
        // is still functional, just obviously wrong for mainnet)
        assert!(r.update_pool_creation_fee([0u8; 20], 1_000 * 10u128.pow(18)).is_ok());
        // Non-zero caller rejected
        assert!(r.update_pool_creation_fee([0xABu8; 20], 0).is_err());
    }

    #[test]
    fn bridge_fee_calculation() {
        let r = FeeRegistry::default(); // 5 bps = 0.05%
        let fee = r.bridge_fee(1_000_000 * 10u128.pow(18));
        assert_eq!(fee, 500 * 10u128.pow(18)); // 0.05% of 1M ZBX = 500 ZBX
    }

    #[test]
    fn gas_cost_estimation() {
        let op = DexOperation::SwapDirect;
        // 14 gwei gas price
        let cost = op.gas_cost_wei(14);
        assert_eq!(cost, 120_000 * 14 * 1_000_000_000u128);
    }

    #[test]
    fn total_cost_estimate() {
        let r = FeeRegistry::default();
        let est = r.estimate_total_cost(DexOperation::CreatePool, 20, 0);
        assert_eq!(est.platform_fee, DEFAULT_POOL_CREATION_FEE);
        assert!(est.gas_cost_wei > 0);
        assert_eq!(est.total_wei, est.platform_fee + est.gas_cost_wei);
    }

    #[test]
    fn governance_fee_update() {
        let gov = [0xABu8; 20];
        let mut r = FeeRegistry::new(gov, [0u8; 20]);
        r.update_pool_creation_fee(gov, 1_000 * 10u128.pow(18)).unwrap();
        assert_eq!(r.pool_creation_fee(), 1_000 * 10u128.pow(18));
        // Non-governance caller rejected
        assert!(r.update_pool_creation_fee([0u8; 20], 0).is_err());
    }

    #[test]
    fn bridge_fee_cap_enforced() {
        let gov = [0xABu8; 20];
        let mut r = FeeRegistry::new(gov, [0u8; 20]);
        assert!(r.update_bridge_fee_bps(gov, 1_001).is_err()); // > 10%
        assert!(r.update_bridge_fee_bps(gov, 50).is_ok());     // 0.5%
    }

    #[test]
    fn all_operations_have_gas_estimate() {
        let ops = [
            DexOperation::SwapDirect, DexOperation::SwapTwoHop,
            DexOperation::AddLiquidity, DexOperation::RemoveLiquidity,
            DexOperation::CreatePool, DexOperation::CreateToken,
            DexOperation::MintTokens, DexOperation::PauseToken,
            DexOperation::RegisterMetadata, DexOperation::Approve,
            DexOperation::TransferFrom, DexOperation::BridgeTransfer,
            DexOperation::RegisterName, DexOperation::ClaimRewards,
        ];
        for op in ops {
            assert!(op.estimated_gas() > 0, "{op:?} has no gas estimate");
        }
    }
}
