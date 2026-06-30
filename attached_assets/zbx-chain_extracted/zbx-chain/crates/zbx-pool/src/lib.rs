//! zbx-pool — Full ZBX DEX: AMM + Swap + Approval + Factory + Token Factory + Fee Registry.
//!
//! # Architecture
//!
//! ```
//!                     ┌──────────────────────────────────────────────────────────┐
//!                     │                    ZBX DEX (zbx-pool)                   │
//!                     │                                                          │
//!  User               │  ┌──────────┐    ┌──────────────────┐  ┌─────────────┐  │
//!   │  buy/sell       │  │  Router  │───►│  Pair (secure)   │  │PoolFactory  │  │
//!   └────────────────►│  │          │    │  reentrancy ✓    │  │  500 ZBX    │  │
//!                     │  │ best     │    │  price impact ✓  │  │  creation   │  │
//!                     │  │ route    │    │  slippage ✓       │  │  fee        │  │
//!                     │  │ (1–2hop) │    │  deadline ✓       │  └─────────────┘  │
//!                     │  └──────────┘    │  oracle check ✓  │                   │
//!                     │                  │  k-invariant ✓   │  ┌─────────────┐  │
//!                     │  ┌────────────┐  │  circuit breaker ✓│  │TokenFactory │  │
//!                     │  │ LP tokens  │  └──────────────────┘  │  100 ZBX    │  │
//!                     │  │ registry   │                         │  creation   │  │
//!                     │  └────────────┘  ┌──────────────────┐  │  fee        │  │
//!                     │                  │  AllowanceReg    │  └─────────────┘  │
//!                     │                  │  ERC-20 approve  │                   │
//!                     │                  │  transferFrom    │  ┌─────────────┐  │
//!                     │                  └──────────────────┘  │ FeeRegistry │  │
//!                     │                                        │  all ops    │  │
//!                     │                  ┌──────────────────┐  └─────────────┘  │
//!                     │                  │   DexEngine      │                   │
//!                     │                  │ buy/sell/liq/    │                   │
//!                     │                  │ create/approve   │                   │
//!                     │                  └──────────────────┘                   │
//!                     └──────────────────────────────────────────────────────────┘
//! ```
//!
//! # Canonical trading pairs
//!
//! | Pair      | Fee   | Description |
//! |-----------|-------|-------------|
//! | ZBX/ZUSD  | 0.30% | ZBX ↔ USD stablecoin |
//!
//! # Fee schedule (platform fees — paid in ZBX)
//!
//! | Operation | Fee |
//! |-----------|-----|
//! | Create pool | 500 ZBX |
//! | Create token | 100 ZBX |
//! | Mint tokens | 1 ZBX/call |
//! | Pause token | 5 ZBX |
//! | Register metadata | 10 ZBX |
//! | Bridge cross-chain | 0.05% of amount |
//!
//! # Security model (swap — 10 sequential checks)
//!
//! 1. Circuit breaker (governance pause)
//! 2. Reentrancy lock
//! 3. Transaction deadline
//! 4. Non-zero input
//! 5. Oracle price sanity (pool vs TWAP ≤ 15%)
//! 6. Price impact cap (≤ 30% per swap)
//! 7. Constant-product formula with fee applied
//! 8. Reserve drain cap (≤ 30% of reserve out per swap)
//! 9. Slippage protection (caller's min_amount_out)
//! 10. k-invariant post-swap check (k must not decrease)

// ── Core AMM modules ──────────────────────────────────────────────────────────
pub mod pair;
pub mod router;
pub mod lp_token;
pub mod fee;
pub mod security;
pub mod error;
pub mod canonical_pairs;

// ── DEX upgrade modules (ZEP-014 v2 + ZEP-026) ───────────────────────────────

/// ERC-20 approve/allowance/transferFrom system.
pub mod approval;

/// Pool factory — create liquidity pools with a paid creation fee.
pub mod factory;

/// Token factory — deploy custom ERC-20 tokens with a paid creation fee.
pub mod token_factory;

/// Platform fee registry — all operation fees + gas estimation.
pub mod registry;

/// DexEngine — top-level buy/sell/liquidity/approval coordinator.
pub mod dex;

// ── Core AMM re-exports ───────────────────────────────────────────────────────

pub use pair::{
    Pair, PairId, SwapParams, SwapResult,
    AddLiquidityParams, AddLiquidityResult,
    RemoveLiquidityParams, RemoveLiquidityResult,
};
pub use router::{
    SwapRoute, SwapStep,
    find_best_route, find_direct_route, execute_route,
};
pub use fee::FeeTier;
pub use lp_token::{LpRegistry, LpError};
pub use security::{
    ReentrancyGuard, CircuitBreaker,
    MIN_LIQUIDITY, MAX_PRICE_IMPACT_BPS, MAX_ORACLE_DEVIATION_BPS,
};
pub use error::AmmError;
pub use canonical_pairs::{
    canonical_pools, CanonicalPool,
    WZBX_ADDR, ZUSD_ADDR,
    POOL_ZBX_ZUSD_ADDR,
    wzbx, zusd,
};

// ── DEX upgrade re-exports ────────────────────────────────────────────────────

pub use approval::{AllowanceRegistry, Approval, ApprovalError};

pub use factory::{
    PoolFactory, PoolRecord, PoolCreatedEvent,
    POOL_CREATION_FEE_WEI, PROTOCOL_TREASURY,
};

pub use token_factory::{
    TokenFactory, TokenRecord, CreateTokenParams, TokenFactoryError,
    MAX_TOKEN_SUPPLY,
};

pub use registry::{
    FeeRegistry, FeeEstimate, DexOperation,
    DEFAULT_POOL_CREATION_FEE, DEFAULT_TOKEN_CREATION_FEE,
    DEFAULT_TOKEN_MINT_FEE, DEFAULT_TOKEN_PAUSE_FEE,
    DEFAULT_METADATA_FEE, DEFAULT_BRIDGE_FEE_BPS,
    DEFAULT_NAME_REGISTRATION_FEE,
};

pub use dex::{
    DexEngine, DexError,
    SwapEvent, LiquidityEvent, QuoteResult,
};
