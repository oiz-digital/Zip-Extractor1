//! Built-in system contracts deployed at genesis.
//!
//! Includes: PayID registry, ZUSD stablecoin,
//! staking escrow, ZEP governance v1 + GovernorV2 with timelock,
//! the native bridge lock contract, genesis pre-mint configuration,
//! and the ZRC-20 v1.1 token state engine (ZEP-006).
//!
//! ## Genesis pre-mint
//!
//! Admin/Foundation treasury is pre-minted at genesis (before block #1):
//!   - ZUSD : 100,000,000 ZUSD (100 million)
//!
//! See `genesis_mint::default_premints()` for the canonical entry list.
//!
//! ## Governance upgrade (Session 35)
//! * `timelock`     — TimelockController: 2-day min delay, guardian veto, predecessor deps
//! * `governor_v2`  — GovernorV2: delegate votes, snapshot power, on-chain call payloads,
//!                    Succeeded → Queued → Executed lifecycle via TimelockController
//!
//! ## ZRC-20 v1.1 token engine (Session 38 — ZEP-006)
//! * `zrc20_token`  — Single-token ZRC-20 v1.1 state machine: ERC-20 core, mintable,
//!                    burnable, freeze (USDC-style), native time-lock, mint enable/disable,
//!                    transfer pause, anti-bot, 2-step ownership, logo URI update.
//!                    Rust mirror of `contracts/ZRC20Token.sol`. All features specified
//!                    in ZEP-006 (Freeze, Native Lock, Mint Enable/Disable) are covered.

pub mod payid;
pub mod zusd;
pub mod staking_escrow;
pub mod governance;
pub mod timelock;
pub mod governor_v2;
pub mod bridge_lock;
pub mod genesis_mint;
pub mod zrc20_token;

pub use genesis_mint::{
    ZBX_ADMIN_ADDR,
    ZUSD_GENESIS_PREMINT,
    TokenPremint,
    default_premints,
    apply_premint,
};

pub use zrc20_token::{
    Zrc20Token,
    Zrc20Error,
    LockInfo,
    TokenInfo,
    DEFAULT_DECIMALS,
    MAX_BATCH_SIZE,
    UNLIMITED_CAP,
};
