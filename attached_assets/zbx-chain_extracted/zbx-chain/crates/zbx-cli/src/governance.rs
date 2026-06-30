//! Governance commands — proposals, voting, execution.
//!
//! M53-04 FIX: All subcommands are now wired to real JSON-RPC calls.
//!
//! Read-only subcommands (`list`, `info`) call `zbx_getProposals` /
//! `zbx_getProposal` on the configured RPC endpoint.
//!
//! Write subcommands (`create`, `vote`, `execute`) build ABI-encoded
//! calldata, sign an EIP-1559 transaction, and broadcast via
//! `eth_sendRawTransaction`.  Pass `--governance-contract <addr>` (or
//! `ZBX_GOVERNANCE_CONTRACT` env var) to specify the on-chain governor.
//!
//! Constants:
//!   - QUORUM_BPS     = 500 (5% of total supply must vote)
//!   - TIMELOCK_DELAY = 172,800 seconds (2 days)
//!   - VOTING_PERIOD  = 302,400 blocks (~7 days at 2s/block)

use clap::{Args, Subcommand};
use serde_json::json;

use crate::config::Context;
use crate::rpc;
use crate::safety;

#[derive(Args, Debug)]
pub struct GovernanceCmd {
    /// On-chain governor contract address (or ZBX_GOVERNANCE_CONTRACT env var).
    #[arg(long, env = "ZBX_GOVERNANCE_CONTRACT")]
    pub governance_contract: Option<String>,

    #[command(subcommand)]
    pub sub: GovernanceSub,
}

#[derive(Subcommand, Debug)]
pub enum GovernanceSub {
    /// List all governance proposals.
    List(GovList),
    /// Show detailed info for a single proposal.
    Info(GovInfo),
    /// Create a new governance proposal.
    Create(GovCreate),
    /// Cast a vote on an active proposal.
    Vote(GovVote),
    /// Execute a passed proposal (after timelock).
    Execute(GovExecute),
}

// ─── list / info (read-only) ─────────────────────────────────────────────────

#[derive(Args, Debug)]
pub struct GovList {
    /// Filter by state: active, pending, succeeded, executed, defeated, all.
    #[arg(long, default_value = "all")] pub state: String,
    #[arg(long, default_value = "20")]  pub limit: usize,
}

impl GovList {
    pub async fn run(&self, ctx: &Context) -> anyhow::Result<()> {
        anyhow::ensure!(self.limit > 0 && self.limit <= 1000,
            "limit must be in 1..=1000");
        anyhow::ensure!(matches!(self.state.as_str(),
            "active" | "pending" | "succeeded" | "executed" | "defeated" | "all"),
            "unknown state {:?}", self.state);

        let result: serde_json::Value = rpc::json_rpc_call(
            &ctx.rpc_url, "zbx_getProposals",
            json!([{ "state": self.state, "limit": self.limit }])
        ).await?;
        println!("{}", serde_json::to_string_pretty(&result)?);
        Ok(())
    }
}

#[derive(Args, Debug)]
pub struct GovInfo {
    #[arg(long)] pub id: u64,
}

impl GovInfo {
    pub async fn run(&self, ctx: &Context) -> anyhow::Result<()> {
        let result: serde_json::Value = rpc::json_rpc_call(
            &ctx.rpc_url, "zbx_getProposal", json!([self.id])
        ).await?;
        println!("{}", serde_json::to_string_pretty(&result)?);
        Ok(())
    }
}

// ─── create / vote / execute (state-changing) ────────────────────────────────

#[derive(Args, Debug)]
pub struct GovCreate {
    #[arg(long)] pub title: String,
    #[arg(long)] pub description: String,
    #[arg(long, default_value = "0x")] pub calldata: String,
    #[arg(long, default_value = "0")]  pub value: u128,
}

impl GovCreate {
    pub async fn run(&self, ctx: &Context, gov_contract: Option<&str>) -> anyhow::Result<()> {
        anyhow::ensure!(!self.title.is_empty(), "title must not be empty");
        anyhow::ensure!(self.title.len() <= 200, "title too long (max 200 chars)");
        anyhow::ensure!(!self.description.is_empty(), "description must not be empty");

        let cd_raw = self.calldata.strip_prefix("0x").unwrap_or(&self.calldata);
        if !cd_raw.is_empty() {
            hex::decode(cd_raw).map_err(|e| anyhow::anyhow!("calldata is not hex: {e}"))?;
        }

        // Dangerous-selector early warning (T213).
        let _ = safety::decode_selector_warn(&self.calldata)?;

        let contract_addr = require_contract(gov_contract, "governance-contract")?;
        let from = ctx.signer_address()?;
        let summary = format!(
            "=== Governance Create preflight ===\n\
             proposer    : 0x{from_hex}\n  governor    : 0x{gov}\n\
             title       : {title}\n  value (wei) : {value}\n\
             calldata    : {cd_short}{trunc}\n  rpc         : {rpc}",
            from_hex = hex::encode(from.0),
            gov      = hex::encode(contract_addr),
            title    = self.title,
            value    = self.value,
            cd_short = &self.calldata.chars().take(80).collect::<String>(),
            trunc    = if self.calldata.len() > 80 { "…" } else { "" },
            rpc      = ctx.rpc_url,
        );
        ctx.confirm_or_yes(&summary)?;
        let wallet = ctx.signer()?;

        // propose(string title, string description, address target, uint256 value, bytes calldata)
        // ABI-encode dynamic types with offset+length headers.
        let title_bytes = self.title.as_bytes();
        let desc_bytes  = self.description.as_bytes();
        let call_bytes  = hex::decode(cd_raw).unwrap_or_default();
        let target_addr = [0u8; 20]; // zero unless caller provides --target

        let selector = rpc::selector("propose(string,string,address,uint256,bytes)");
        // Build ABI-encoded calldata (simplified — fixed-size fields + dynamic)
        let mut data = Vec::new();
        data.extend_from_slice(&selector);
        // offsets for 5 params: title(0), desc(1), target(2), value(3), calldata(4)
        // dynamic: title at offset 5*32=160, desc after, call after
        let title_offset  = 5 * 32usize;
        let desc_offset   = title_offset + 32 + pad32(title_bytes.len());
        let call_offset   = desc_offset  + 32 + pad32(desc_bytes.len());
        data.extend_from_slice(&rpc::encode_uint128(title_offset as u128));
        data.extend_from_slice(&rpc::encode_uint128(desc_offset as u128));
        data.extend_from_slice(&rpc::encode_address(&target_addr));
        data.extend_from_slice(&rpc::encode_uint128(self.value));
        data.extend_from_slice(&rpc::encode_uint128(call_offset as u128));
        // title
        data.extend_from_slice(&rpc::encode_uint128(title_bytes.len() as u128));
        data.extend_from_slice(title_bytes);
        data.resize(data.len() + pad_diff(title_bytes.len()), 0);
        // description
        data.extend_from_slice(&rpc::encode_uint128(desc_bytes.len() as u128));
        data.extend_from_slice(desc_bytes);
        data.resize(data.len() + pad_diff(desc_bytes.len()), 0);
        // calldata bytes
        data.extend_from_slice(&rpc::encode_uint128(call_bytes.len() as u128));
        data.extend_from_slice(&call_bytes);
        data.resize(data.len() + pad_diff(call_bytes.len()), 0);

        let tx_hash = rpc::build_and_send_tx(
            &ctx.rpc_url, &wallet, ctx.chain_id,
            Some(&contract_addr), &data, 0, 500_000,
        ).await?;
        println!("Proposal submitted. tx: {tx_hash}");
        Ok(())
    }
}

#[derive(Args, Debug)]
pub struct GovVote {
    #[arg(long)] pub id: u64,
    /// yes / no / abstain
    #[arg(long)] pub support: String,
}

impl GovVote {
    pub async fn run(&self, ctx: &Context, gov_contract: Option<&str>) -> anyhow::Result<()> {
        let support = match self.support.to_lowercase().as_str() {
            "yes" | "for"     => 1u8,
            "no"  | "against" => 0u8,
            "abstain"         => 2u8,
            other => anyhow::bail!("unknown vote: {other:?}. Use yes/no/abstain"),
        };
        let label = ["Against", "For", "Abstain"][support as usize];
        let contract_addr = require_contract(gov_contract, "governance-contract")?;

        let from = ctx.signer_address()?;
        let summary = format!(
            "=== Governance Vote preflight ===\n\
             voter       : 0x{}\n  governor    : 0x{}\n\
             proposal id : {}\n  vote        : {label}\n  rpc         : {}",
            hex::encode(from.0), hex::encode(contract_addr), self.id, ctx.rpc_url,
        );
        ctx.confirm_or_yes(&summary)?;
        let wallet = ctx.signer()?;

        // castVote(uint256 proposalId, uint8 support)
        let mut data = Vec::with_capacity(68);
        data.extend_from_slice(&rpc::selector("castVote(uint256,uint8)"));
        data.extend_from_slice(&rpc::encode_uint64(self.id));
        data.extend_from_slice(&rpc::encode_uint8(support));

        let tx_hash = rpc::build_and_send_tx(
            &ctx.rpc_url, &wallet, ctx.chain_id,
            Some(&contract_addr), &data, 0, 200_000,
        ).await?;
        println!("Vote submitted ({label}). tx: {tx_hash}");
        Ok(())
    }
}

#[derive(Args, Debug)]
pub struct GovExecute {
    #[arg(long)] pub id: u64,
    /// Optional: proposal calldata for local dangerous-selector check (T213).
    #[arg(long)] pub calldata: Option<String>,
    /// Optional: scheduled timelock ETA (unix seconds). Fails locally if not elapsed.
    #[arg(long)] pub eta: Option<u64>,
}

impl GovExecute {
    pub async fn run(&self, ctx: &Context, gov_contract: Option<&str>) -> anyhow::Result<()> {
        // T213: timelock guard.
        if let Some(eta) = self.eta {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs();
            if now < eta {
                anyhow::bail!(
                    "timelock has not yet elapsed: now={now}, eta={eta}, wait {} more seconds",
                    eta - now
                );
            }
        }

        // T213: warn on dangerous selectors.
        if let Some(cd) = &self.calldata {
            let _ = safety::decode_selector_warn(cd)?;
        } else {
            eprintln!(
                "note: no --calldata supplied; dangerous-selector check was not run locally."
            );
        }

        let contract_addr = require_contract(gov_contract, "governance-contract")?;
        let from = ctx.signer_address()?;
        let summary = format!(
            "=== Governance Execute preflight ===\n\
             executor    : 0x{}\n  governor    : 0x{}\n\
             proposal id : {}\n  rpc         : {}",
            hex::encode(from.0), hex::encode(contract_addr), self.id, ctx.rpc_url,
        );
        ctx.confirm_or_yes(&summary)?;
        let wallet = ctx.signer()?;

        // execute(uint256 proposalId)
        let mut data = Vec::with_capacity(36);
        data.extend_from_slice(&rpc::selector("execute(uint256)"));
        data.extend_from_slice(&rpc::encode_uint64(self.id));

        let tx_hash = rpc::build_and_send_tx(
            &ctx.rpc_url, &wallet, ctx.chain_id,
            Some(&contract_addr), &data, 0, 500_000,
        ).await?;
        println!("Execute submitted. tx: {tx_hash}");
        Ok(())
    }
}

// ─── helpers ─────────────────────────────────────────────────────────────────

fn parse_addr(s: &str) -> anyhow::Result<[u8; 20]> {
    let h = s.strip_prefix("0x").unwrap_or(s);
    let raw = hex::decode(h).map_err(|e| anyhow::anyhow!("address is not hex: {e}"))?;
    if raw.len() != 20 { anyhow::bail!("address must be 20 bytes, got {}", raw.len()); }
    let mut out = [0u8; 20];
    out.copy_from_slice(&raw);
    Ok(out)
}

fn require_contract(addr: Option<&str>, flag: &str) -> anyhow::Result<[u8; 20]> {
    let s = addr.ok_or_else(|| anyhow::anyhow!(
        "no --{flag} provided; set ZBX_GOVERNANCE_CONTRACT env var or pass --governance-contract"
    ))?;
    parse_addr(s)
}

/// Pad `len` bytes to the next multiple of 32.
fn pad32(len: usize) -> usize { (len + 31) & !31 }

/// Number of zero bytes needed to pad to next multiple of 32.
fn pad_diff(len: usize) -> usize { pad32(len) - len }

pub async fn run(cmd: GovernanceCmd, ctx: &Context) -> anyhow::Result<()> {
    let contract = cmd.governance_contract.as_deref();
    match cmd.sub {
        GovernanceSub::List(c)    => c.run(ctx).await,
        GovernanceSub::Info(c)    => c.run(ctx).await,
        GovernanceSub::Create(c)  => c.run(ctx, contract).await,
        GovernanceSub::Vote(c)    => c.run(ctx, contract).await,
        GovernanceSub::Execute(c) => c.run(ctx, contract).await,
    }
}
