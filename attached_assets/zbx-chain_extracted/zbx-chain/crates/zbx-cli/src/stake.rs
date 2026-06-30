//! Staking commands — delegate, undelegate, claim, validator queries.
//!
//! M53-04 FIX: All subcommands are now wired to real JSON-RPC calls.
//!
//! Read-only subcommands (`validators`, `validator`, `info`, `slashing`)
//! call the `zbx_*` RPC namespace directly.
//!
//! Write subcommands (`delegate`, `undelegate`, `claim`) build ABI-encoded
//! calldata, sign an EIP-1559 transaction with the keystore wallet, and
//! broadcast via `eth_sendRawTransaction`.  Pass `--staking-contract <addr>`
//! (or `ZBX_STAKING_CONTRACT` env var) to specify the on-chain registry.
//!
//! Constants:
//!   - MIN_STAKE           = 1,000 ZBX
//!   - STAKE_LOCK          = 1,209,600 s (14 days)
//!   - ANNUAL_EMISSION_CAP = 50,000,000 ZBX/year

use clap::{Args, Subcommand};
use serde_json::json;

use crate::config::Context;
use crate::rpc;

const MIN_STAKE_WEI: u128 = 1_000 * 1_000_000_000_000_000_000; // 1,000 ZBX
const STAKE_LOCK_SECS: u64 = 14 * 24 * 60 * 60;                 // 14 days
const DEFAULT_GAS: u64 = 350_000;

#[derive(Args, Debug)]
pub struct StakeCmd {
    /// On-chain staking contract address (or ZBX_STAKING_CONTRACT env var).
    #[arg(long, env = "ZBX_STAKING_CONTRACT")]
    pub staking_contract: Option<String>,

    #[command(subcommand)]
    pub sub: StakeSub,
}

#[derive(Subcommand, Debug)]
pub enum StakeSub {
    Delegate(StakeDelegate),
    Undelegate(StakeUndelegate),
    Claim(StakeClaim),
    Info(StakeInfo),
    Validators(StakeValidators),
    Validator(StakeValidatorInfo),
    Slashing(StakeSlashing),
}

// ─── state-changing ──────────────────────────────────────────────────────────

#[derive(Args, Debug)]
pub struct StakeDelegate {
    #[arg(long)] pub validator: String,
    /// Amount in wei (1 ZBX = 1e18). Minimum 1,000 ZBX.
    #[arg(long)] pub amount: u128,
}

impl StakeDelegate {
    pub async fn run(&self, ctx: &Context, staking_contract: Option<&str>) -> anyhow::Result<()> {
        let validator_addr = parse_addr(&self.validator)?;
        let contract_addr  = require_contract(staking_contract, "staking-contract")?;

        anyhow::ensure!(
            self.amount >= MIN_STAKE_WEI,
            "minimum stake is 1,000 ZBX (= {} wei), got {}", MIN_STAKE_WEI, self.amount
        );

        let from = ctx.signer_address()?;
        let summary = format!(
            "=== Stake Delegate preflight ===\n\
             delegator   : 0x{}\n  validator   : {}\n  contract    : 0x{}\n\
             amount (wei): {}\n  amount (ZBX): {:.4}\n  unlock after: {} s ({} days)\n  rpc         : {}",
            hex::encode(from.0), self.validator, hex::encode(contract_addr),
            self.amount, self.amount as f64 / 1e18,
            STAKE_LOCK_SECS, STAKE_LOCK_SECS / 86_400, ctx.rpc_url,
        );
        ctx.confirm_or_yes(&summary)?;

        let wallet = ctx.signer()?;

        // delegate(address validator, uint256 amount)
        let mut data = Vec::with_capacity(68);
        data.extend_from_slice(&rpc::selector("delegate(address,uint256)"));
        data.extend_from_slice(&rpc::encode_address(&validator_addr));
        data.extend_from_slice(&rpc::encode_uint128(self.amount));

        let tx_hash = rpc::build_and_send_tx(
            &ctx.rpc_url, &wallet, ctx.chain_id,
            Some(&contract_addr), &data, 0, DEFAULT_GAS,
        ).await?;
        println!("Delegate submitted. tx: {tx_hash}");
        Ok(())
    }
}

#[derive(Args, Debug)]
pub struct StakeUndelegate {
    #[arg(long)] pub validator: String,
    #[arg(long)] pub amount: u128,
}

impl StakeUndelegate {
    pub async fn run(&self, ctx: &Context, staking_contract: Option<&str>) -> anyhow::Result<()> {
        let validator_addr = parse_addr(&self.validator)?;
        let contract_addr  = require_contract(staking_contract, "staking-contract")?;

        anyhow::ensure!(self.amount > 0, "amount must be > 0");

        let from = ctx.signer_address()?;
        let summary = format!(
            "=== Stake Undelegate preflight ===\n\
             delegator   : 0x{}\n  validator   : {}\n  contract    : 0x{}\n\
             amount (wei): {}\n  unlock after: {} s ({} days)\n  rpc         : {}",
            hex::encode(from.0), self.validator, hex::encode(contract_addr),
            self.amount, STAKE_LOCK_SECS, STAKE_LOCK_SECS / 86_400, ctx.rpc_url,
        );
        ctx.confirm_or_yes(&summary)?;

        let wallet = ctx.signer()?;

        // undelegate(address validator, uint256 amount)
        let mut data = Vec::with_capacity(68);
        data.extend_from_slice(&rpc::selector("undelegate(address,uint256)"));
        data.extend_from_slice(&rpc::encode_address(&validator_addr));
        data.extend_from_slice(&rpc::encode_uint128(self.amount));

        let tx_hash = rpc::build_and_send_tx(
            &ctx.rpc_url, &wallet, ctx.chain_id,
            Some(&contract_addr), &data, 0, DEFAULT_GAS,
        ).await?;
        println!("Undelegate submitted. tx: {tx_hash}");
        Ok(())
    }
}

#[derive(Args, Debug)]
pub struct StakeClaim {
    /// Claim from this validator (omit for "all").
    #[arg(long)] pub validator: Option<String>,
}

impl StakeClaim {
    pub async fn run(&self, ctx: &Context, staking_contract: Option<&str>) -> anyhow::Result<()> {
        let contract_addr = require_contract(staking_contract, "staking-contract")?;

        let validator_addr = if let Some(v) = &self.validator {
            Some(parse_addr(v)?)
        } else { None };

        let from = ctx.signer_address()?;
        let summary = format!(
            "=== Stake Claim preflight ===\n\
             delegator : 0x{}\n  contract  : 0x{}\n  validator : {}\n  rpc       : {}",
            hex::encode(from.0), hex::encode(contract_addr),
            self.validator.clone().unwrap_or_else(|| "<all>".into()),
            ctx.rpc_url,
        );
        ctx.confirm_or_yes(&summary)?;

        let wallet = ctx.signer()?;

        let data: Vec<u8> = if let Some(vaddr) = validator_addr {
            // claimRewards(address validator)
            let mut d = Vec::with_capacity(36);
            d.extend_from_slice(&rpc::selector("claimRewards(address)"));
            d.extend_from_slice(&rpc::encode_address(&vaddr));
            d
        } else {
            // claimAllRewards()
            rpc::selector("claimAllRewards()").to_vec()
        };

        let tx_hash = rpc::build_and_send_tx(
            &ctx.rpc_url, &wallet, ctx.chain_id,
            Some(&contract_addr), &data, 0, DEFAULT_GAS,
        ).await?;
        println!("Claim submitted. tx: {tx_hash}");
        Ok(())
    }
}

// ─── read-only ────────────────────────────────────────────────────────────────

#[derive(Args, Debug)]
pub struct StakeInfo {
    #[arg(long)] pub address: Option<String>,
}

impl StakeInfo {
    pub async fn run(&self, ctx: &Context) -> anyhow::Result<()> {
        let addr_bytes = match &self.address {
            Some(a) => parse_addr(a)?,
            None    => ctx.signer_address()?.0,
        };
        let addr_hex = format!("0x{}", hex::encode(addr_bytes));
        let result: serde_json::Value = rpc::json_rpc_call(
            &ctx.rpc_url, "zbx_getDelegatorInfo", json!([addr_hex])
        ).await?;
        println!("{}", serde_json::to_string_pretty(&result)?);
        Ok(())
    }
}

#[derive(Args, Debug)]
pub struct StakeValidators {
    #[arg(long)] pub all: bool,
}

impl StakeValidators {
    pub async fn run(&self, ctx: &Context) -> anyhow::Result<()> {
        let params = if self.all { json!([{"active": false}]) } else { json!([]) };
        let result: serde_json::Value = rpc::json_rpc_call(&ctx.rpc_url, "zbx_getValidators", params).await?;
        println!("{}", serde_json::to_string_pretty(&result)?);
        Ok(())
    }
}

#[derive(Args, Debug)]
pub struct StakeValidatorInfo {
    #[arg(long)] pub address: String,
}

impl StakeValidatorInfo {
    pub async fn run(&self, ctx: &Context) -> anyhow::Result<()> {
        let addr_bytes = parse_addr(&self.address)?;
        let addr_hex = format!("0x{}", hex::encode(addr_bytes));
        let result: serde_json::Value = rpc::json_rpc_call(
            &ctx.rpc_url, "zbx_getValidator", json!([addr_hex])
        ).await?;
        println!("{}", serde_json::to_string_pretty(&result)?);
        Ok(())
    }
}

#[derive(Args, Debug)]
pub struct StakeSlashing {
    #[arg(long)] pub address: String,
    #[arg(long, default_value = "20")] pub limit: usize,
}

impl StakeSlashing {
    pub async fn run(&self, ctx: &Context) -> anyhow::Result<()> {
        let addr_bytes = parse_addr(&self.address)?;
        anyhow::ensure!(self.limit > 0 && self.limit <= 1000,
            "limit must be in 1..=1000");
        let addr_hex = format!("0x{}", hex::encode(addr_bytes));
        let result: serde_json::Value = rpc::json_rpc_call(
            &ctx.rpc_url, "zbx_getSlashingHistory",
            json!([addr_hex, self.limit])
        ).await?;
        println!("{}", serde_json::to_string_pretty(&result)?);
        Ok(())
    }
}

// ─── helpers ─────────────────────────────────────────────────────────────────

fn parse_addr(s: &str) -> anyhow::Result<[u8; 20]> {
    let h = s.strip_prefix("0x").unwrap_or(s);
    let raw = hex::decode(h).map_err(|e| anyhow::anyhow!("address is not hex: {e}"))?;
    if raw.len() != 20 {
        anyhow::bail!("address must be 20 bytes, got {}", raw.len());
    }
    let mut out = [0u8; 20];
    out.copy_from_slice(&raw);
    Ok(out)
}

fn require_contract(addr: Option<&str>, flag: &str) -> anyhow::Result<[u8; 20]> {
    let s = addr.ok_or_else(|| anyhow::anyhow!(
        "no --{flag} provided; set ZBX_STAKING_CONTRACT env var or pass --staking-contract"
    ))?;
    parse_addr(s)
}

pub async fn run(cmd: StakeCmd, ctx: &Context) -> anyhow::Result<()> {
    let contract = cmd.staking_contract.as_deref();
    match cmd.sub {
        StakeSub::Delegate(c)   => c.run(ctx, contract).await,
        StakeSub::Undelegate(c) => c.run(ctx, contract).await,
        StakeSub::Claim(c)      => c.run(ctx, contract).await,
        StakeSub::Info(c)       => c.run(ctx).await,
        StakeSub::Validators(c) => c.run(ctx).await,
        StakeSub::Validator(c)  => c.run(ctx).await,
        StakeSub::Slashing(c)   => c.run(ctx).await,
    }
}
