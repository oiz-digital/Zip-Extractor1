//! DeFi commands — token operations, swap quote/execute, liquidity, oracle.
//!
//! M53-04 FIX: `oracle-price` and `token-balance` are now wired to real
//! `eth_call` requests.  Swap/liquidity/transfer/approve write commands still
//! run the full preflight and are ready to be wired once router addresses are
//! standardised — the scaffolding (calldata encoding + RPC broadcast) is in
//! place.  TVL was already wired in a previous session and is unchanged.

use clap::{Args, Subcommand};
use serde_json::json;
use sha3::{Digest, Keccak256};
use zbx_types::U256;

use crate::config::Context;
use crate::rpc;
use crate::safety;

#[derive(Args, Debug)]
pub struct DefiCmd {
    #[command(subcommand)]
    pub sub: DefiSub,
}

#[derive(Subcommand, Debug)]
pub enum DefiSub {
    TokenBalance(TokenBalance),
    TokenTransfer(TokenTransfer),
    TokenApprove(TokenApprove),
    SwapQuote(SwapQuote),
    SwapExecute(SwapExecute),
    AddLiquidity(AddLiquidity),
    RemoveLiquidity(RemoveLiquidity),
    OraclePrice(OraclePrice),
    Tvl(Tvl),
}

// ─── tokens ──────────────────────────────────────────────────────────────────

#[derive(Args, Debug)]
pub struct TokenBalance {
    #[arg(long)] pub token: String,
    #[arg(long)] pub address: Option<String>,
}

impl TokenBalance {
    pub async fn run(&self, ctx: &Context) -> anyhow::Result<()> {
        let token_addr = parse_addr(&self.token)?;
        let query_addr = match &self.address {
            Some(a) => parse_addr(a)?,
            None    => ctx.signer_address()?.0,
        };

        // ERC-20 balanceOf(address) selector
        let mut call_data = Vec::with_capacity(36);
        call_data.extend_from_slice(&rpc::selector("balanceOf(address)"));
        call_data.extend_from_slice(&rpc::encode_address(&query_addr));

        let result = rpc::eth_call_raw(&ctx.rpc_url, &token_addr, &call_data).await?;

        if result.len() < 32 {
            anyhow::bail!("balanceOf returned {} bytes (expected 32)", result.len());
        }
        let balance = U256::from_big_endian(&result[..32]);
        println!("Token:   0x{}", hex::encode(token_addr));
        println!("Account: 0x{}", hex::encode(query_addr));
        println!("Balance: {} (raw wei)", balance);
        println!("Balance: {:.6} (18-decimal)", balance.to_string().parse::<f64>().unwrap_or(0.0) / 1e18);
        Ok(())
    }
}

#[derive(Args, Debug)]
pub struct TokenTransfer {
    #[arg(long)] pub token: String,
    #[arg(long)] pub to: String,
    #[arg(long)] pub amount: u128,
}

impl TokenTransfer {
    pub async fn run(&self, ctx: &Context) -> anyhow::Result<()> {
        let token_addr = parse_addr(&self.token)?;
        let to_addr    = parse_addr(&self.to)?;
        anyhow::ensure!(self.amount > 0, "amount must be > 0");

        let from = ctx.signer_address()?;
        let summary = format!(
            "=== ERC-20 Transfer preflight ===\n\
             token   : 0x{}\n  from    : 0x{}\n  to      : 0x{}\n  amount  : {} (raw)",
            hex::encode(token_addr), hex::encode(from.0), hex::encode(to_addr), self.amount,
        );
        ctx.confirm_or_yes(&summary)?;
        let wallet = ctx.signer()?;

        // transfer(address to, uint256 amount)
        let mut data = Vec::with_capacity(68);
        data.extend_from_slice(&rpc::selector("transfer(address,uint256)"));
        data.extend_from_slice(&rpc::encode_address(&to_addr));
        data.extend_from_slice(&rpc::encode_uint128(self.amount));

        let tx_hash = rpc::build_and_send_tx(
            &ctx.rpc_url, &wallet, ctx.chain_id,
            Some(&token_addr), &data, 0, 80_000,
        ).await?;
        println!("Transfer submitted. tx: {tx_hash}");
        Ok(())
    }
}

#[derive(Args, Debug)]
pub struct TokenApprove {
    #[arg(long)] pub token: String,
    #[arg(long)] pub spender: String,
    /// Amount in wei, or "max" for u128::MAX.
    #[arg(long)] pub amount: String,
}

impl TokenApprove {
    pub async fn run(&self, ctx: &Context) -> anyhow::Result<()> {
        let token_addr   = parse_addr(&self.token)?;
        let spender_addr = parse_addr(&self.spender)?;
        let amount: u128 = if self.amount.eq_ignore_ascii_case("max") {
            u128::MAX
        } else {
            self.amount.parse()
                .map_err(|e| anyhow::anyhow!("invalid amount: {e}"))?
        };
        anyhow::ensure!(amount > 0, "amount must be > 0 (or 'max')");

        let from = ctx.signer_address()?;
        let summary = format!(
            "=== ERC-20 Approve preflight ===\n\
             token   : 0x{}\n  owner   : 0x{}\n  spender : 0x{}\n  amount  : {}{}",
            hex::encode(token_addr), hex::encode(from.0), hex::encode(spender_addr),
            amount, if amount == u128::MAX { "  (UNLIMITED)" } else { "" },
        );
        ctx.confirm_or_yes(&summary)?;
        let wallet = ctx.signer()?;

        // approve(address spender, uint256 amount)
        let mut data = Vec::with_capacity(68);
        data.extend_from_slice(&rpc::selector("approve(address,uint256)"));
        data.extend_from_slice(&rpc::encode_address(&spender_addr));
        data.extend_from_slice(&rpc::encode_uint128(amount));

        let tx_hash = rpc::build_and_send_tx(
            &ctx.rpc_url, &wallet, ctx.chain_id,
            Some(&token_addr), &data, 0, 80_000,
        ).await?;
        println!("Approve submitted. tx: {tx_hash}");
        Ok(())
    }
}

// ─── swap ─────────────────────────────────────────────────────────────────────

#[derive(Args, Debug)]
pub struct SwapQuote {
    #[arg(long)] pub token_in: String,
    #[arg(long)] pub token_out: String,
    #[arg(long)] pub amount_in: u128,
    #[arg(long, default_value = "30")] pub fee_bps: u32,
}

impl SwapQuote {
    pub async fn run(&self, ctx: &Context) -> anyhow::Result<()> {
        let token_in  = parse_addr(&self.token_in)?;
        let token_out = parse_addr(&self.token_out)?;
        anyhow::ensure!(self.amount_in > 0, "amount-in must be > 0");
        anyhow::ensure!(
            matches!(self.fee_bps, 5 | 30 | 100),
            "fee-bps must be one of 5 / 30 / 100, got {}", self.fee_bps,
        );
        let result: serde_json::Value = rpc::json_rpc_call(&ctx.rpc_url, "zbx_getSwapQuote", json!([{
            "tokenIn":  format!("0x{}", hex::encode(token_in)),
            "tokenOut": format!("0x{}", hex::encode(token_out)),
            "amountIn": format!("{}", self.amount_in),
            "feeBps":   self.fee_bps,
        }])).await?;
        println!("{}", serde_json::to_string_pretty(&result)?);
        Ok(())
    }
}

#[derive(Args, Debug)]
pub struct SwapExecute {
    #[arg(long)] pub token_in: String,
    #[arg(long)] pub token_out: String,
    #[arg(long)] pub amount_in: u128,
    /// Slippage tolerance in basis points. Clamped to [1, 1000] (T212).
    #[arg(long, default_value = "50")] pub slippage: u32,
    #[arg(long, default_value = "300")] pub deadline: u64,
    #[arg(long, default_value = "30")] pub fee_bps: u32,
    #[arg(long)] pub recipient: Option<String>,
    /// ZbxRouter contract address (or ZBX_ROUTER_CONTRACT env var).
    #[arg(long, env = "ZBX_ROUTER_CONTRACT")] pub router: Option<String>,
}

impl SwapExecute {
    pub async fn run(&self, ctx: &Context) -> anyhow::Result<()> {
        let token_in  = parse_addr(&self.token_in)?;
        let token_out = parse_addr(&self.token_out)?;
        if let Some(r) = &self.recipient { let _ = parse_addr(r)?; }
        anyhow::ensure!(self.amount_in > 0, "amount-in must be > 0");
        anyhow::ensure!(self.deadline >= 30, "deadline must be at least 30 seconds");

        let slippage = safety::slippage_clamp(self.slippage)?;
        let from = ctx.signer_address()?;
        let recipient_addr = match &self.recipient {
            Some(r) => parse_addr(r)?,
            None    => from.0,
        };
        let router_addr = parse_addr(
            self.router.as_deref().ok_or_else(|| anyhow::anyhow!(
                "no --router provided; set ZBX_ROUTER_CONTRACT env var"
            ))?
        )?;

        let summary = format!(
            "=== Swap Execute preflight ===\n\
             from         : 0x{from_hex}\n  router       : 0x{router}\n\
             token_in     : 0x{tin}\n  token_out    : 0x{tout}\n\
             amount_in    : {amt} (raw)\n  fee tier     : {fee} bps\n\
             slippage     : {slip} bps  ({slip_pct}%)\n\
             deadline     : {dl} s from now\n  recipient    : 0x{recip}",
            from_hex = hex::encode(from.0),
            router   = hex::encode(router_addr),
            tin      = hex::encode(token_in),
            tout     = hex::encode(token_out),
            amt      = self.amount_in,
            fee      = self.fee_bps,
            slip     = slippage,
            slip_pct = slippage as f64 / 100.0,
            dl       = self.deadline,
            recip    = hex::encode(recipient_addr),
        );
        ctx.confirm_or_yes(&summary)?;
        let wallet = ctx.signer()?;

        let deadline_abs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?.as_secs()
            + self.deadline;

        // swapExactIn(address tokenIn, address tokenOut, uint256 amountIn,
        //              uint256 minAmountOut, address recipient, uint256 deadline)
        let min_out = self.amount_in * (10_000 - slippage as u128) / 10_000;
        let mut data = Vec::with_capacity(196);
        data.extend_from_slice(&rpc::selector(
            "swapExactIn(address,address,uint256,uint256,address,uint256)"
        ));
        data.extend_from_slice(&rpc::encode_address(&token_in));
        data.extend_from_slice(&rpc::encode_address(&token_out));
        data.extend_from_slice(&rpc::encode_uint128(self.amount_in));
        data.extend_from_slice(&rpc::encode_uint128(min_out));
        data.extend_from_slice(&rpc::encode_address(&recipient_addr));
        data.extend_from_slice(&rpc::encode_uint64(deadline_abs));

        let tx_hash = rpc::build_and_send_tx(
            &ctx.rpc_url, &wallet, ctx.chain_id,
            Some(&router_addr), &data, 0, 300_000,
        ).await?;
        println!("Swap submitted. tx: {tx_hash}");
        Ok(())
    }
}

// ─── liquidity ────────────────────────────────────────────────────────────────

#[derive(Args, Debug)]
pub struct AddLiquidity {
    #[arg(long)] pub pool: String,
    #[arg(long)] pub amount0: u128,
    #[arg(long)] pub amount1: u128,
    #[arg(long, default_value = "50")] pub slippage: u32,
}

impl AddLiquidity {
    pub async fn run(&self, ctx: &Context) -> anyhow::Result<()> {
        let pool_addr = parse_addr(&self.pool)?;
        anyhow::ensure!(self.amount0 > 0 && self.amount1 > 0, "amounts must be > 0");
        let slippage = safety::slippage_clamp(self.slippage)?;

        let from = ctx.signer_address()?;
        let summary = format!(
            "=== Add Liquidity preflight ===\n\
             pool      : 0x{}\n  from      : 0x{}\n\
             amount0   : {}\n  amount1   : {}\n  slippage  : {} bps",
            hex::encode(pool_addr), hex::encode(from.0),
            self.amount0, self.amount1, slippage,
        );
        ctx.confirm_or_yes(&summary)?;
        let wallet = ctx.signer()?;

        let min0 = self.amount0 * (10_000 - slippage as u128) / 10_000;
        let min1 = self.amount1 * (10_000 - slippage as u128) / 10_000;
        let deadline_abs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?.as_secs() + 300;

        // addLiquidity(uint256 amount0, uint256 amount1, uint256 min0, uint256 min1, uint256 deadline)
        let mut data = Vec::with_capacity(164);
        data.extend_from_slice(&rpc::selector(
            "addLiquidity(uint256,uint256,uint256,uint256,uint256)"
        ));
        data.extend_from_slice(&rpc::encode_uint128(self.amount0));
        data.extend_from_slice(&rpc::encode_uint128(self.amount1));
        data.extend_from_slice(&rpc::encode_uint128(min0));
        data.extend_from_slice(&rpc::encode_uint128(min1));
        data.extend_from_slice(&rpc::encode_uint64(deadline_abs));

        let tx_hash = rpc::build_and_send_tx(
            &ctx.rpc_url, &wallet, ctx.chain_id,
            Some(&pool_addr), &data, 0, 300_000,
        ).await?;
        println!("Add liquidity submitted. tx: {tx_hash}");
        Ok(())
    }
}

#[derive(Args, Debug)]
pub struct RemoveLiquidity {
    #[arg(long)] pub pool: String,
    #[arg(long)] pub lp_tokens: u128,
    #[arg(long, default_value = "50")] pub slippage: u32,
}

impl RemoveLiquidity {
    pub async fn run(&self, ctx: &Context) -> anyhow::Result<()> {
        let pool_addr = parse_addr(&self.pool)?;
        anyhow::ensure!(self.lp_tokens > 0, "lp-tokens must be > 0");
        let slippage = safety::slippage_clamp(self.slippage)?;

        let from = ctx.signer_address()?;
        let summary = format!(
            "=== Remove Liquidity preflight ===\n\
             pool       : 0x{}\n  from       : 0x{}\n\
             lp tokens  : {}\n  slippage   : {} bps",
            hex::encode(pool_addr), hex::encode(from.0), self.lp_tokens, slippage,
        );
        ctx.confirm_or_yes(&summary)?;
        let wallet = ctx.signer()?;

        let deadline_abs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?.as_secs() + 300;

        // removeLiquidity(uint256 lpTokens, uint256 min0, uint256 min1, uint256 deadline)
        let mut data = Vec::with_capacity(132);
        data.extend_from_slice(&rpc::selector(
            "removeLiquidity(uint256,uint256,uint256,uint256)"
        ));
        data.extend_from_slice(&rpc::encode_uint128(self.lp_tokens));
        data.extend_from_slice(&rpc::encode_uint128(0)); // min0 = 0 (slippage on quote)
        data.extend_from_slice(&rpc::encode_uint128(0)); // min1 = 0
        data.extend_from_slice(&rpc::encode_uint64(deadline_abs));

        let tx_hash = rpc::build_and_send_tx(
            &ctx.rpc_url, &wallet, ctx.chain_id,
            Some(&pool_addr), &data, 0, 300_000,
        ).await?;
        println!("Remove liquidity submitted. tx: {tx_hash}");
        Ok(())
    }
}

// ─── oracle ────────────────────────────────────────────────────────────────────

#[derive(Args, Debug)]
pub struct OraclePrice {
    /// Feed name e.g. ZBX/USD, ETH/USD, BTC/USD.
    #[arg(long)] pub feed: String,
    /// Oracle contract address (or ZBX_ORACLE_CONTRACT env var).
    #[arg(long, env = "ZBX_ORACLE_CONTRACT")] pub oracle: Option<String>,
}

impl OraclePrice {
    pub async fn run(&self, ctx: &Context) -> anyhow::Result<()> {
        anyhow::ensure!(!self.feed.is_empty(), "feed name must not be empty");
        anyhow::ensure!(self.feed.contains('/'), "feed must contain '/' (e.g. ZBX/USD)");

        let oracle_addr = parse_addr(
            self.oracle.as_deref().ok_or_else(|| anyhow::anyhow!(
                "no --oracle provided; set ZBX_ORACLE_CONTRACT env var"
            ))?
        )?;

        // Try zbx_getOraclePrice first (native RPC namespace).
        let price_result = rpc::json_rpc_call(
            &ctx.rpc_url, "zbx_getOraclePrice", json!([self.feed])
        ).await;

        if let Ok(price) = price_result {
            println!("Feed:   {}", self.feed);
            println!("Oracle: 0x{}", hex::encode(oracle_addr));
            println!("Price:  {}", serde_json::to_string_pretty(&price)?);
            return Ok(());
        }

        // Fallback: eth_call getPrice(address) on the oracle contract.
        // Compute feed address from keccak256 of feed name (convention on ZBX).
        let feed_hash = Keccak256::digest(self.feed.as_bytes());
        let mut feed_addr = [0u8; 20];
        feed_addr.copy_from_slice(&feed_hash[12..]);

        let mut call_data = Vec::with_capacity(36);
        call_data.extend_from_slice(&rpc::selector("getPrice(address)"));
        call_data.extend_from_slice(&rpc::encode_address(&feed_addr));

        let raw = rpc::eth_call_raw(&ctx.rpc_url, &oracle_addr, &call_data).await?;
        if raw.len() < 32 {
            anyhow::bail!("getPrice returned {} bytes (expected 32)", raw.len());
        }
        let price = U256::from_big_endian(&raw[..32]);
        println!("Feed:    {}", self.feed);
        println!("Oracle:  0x{}", hex::encode(oracle_addr));
        println!("Price:   {} (raw, 8-decimal)", price);
        println!("Price $: {:.4}", price.low_u128() as f64 / 1e8);
        Ok(())
    }
}

// ─── tvl (already wired from previous session) ────────────────────────────────

#[derive(Args, Debug)]
pub struct Tvl {
    /// Oracle contract address.
    #[arg(long)] pub oracle: String,
    /// Emit JSON instead of the human-readable table.
    #[arg(long, default_value_t = false)] pub json: bool,
}

impl Tvl {
    pub async fn run(&self, ctx: &Context) -> anyhow::Result<()> {
        let oracle = parse_addr(&self.oracle)?;

        let mut hasher = Keccak256::new();
        hasher.update(b"tvlBreakdown()");
        let digest   = hasher.finalize();
        let selector = &digest[..4];
        let calldata = format!("0x{}", hex::encode(selector));
        let to_hex   = format!("0x{}", hex::encode(oracle));

        let body = serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "method": "eth_call",
            "params": [{"to": to_hex, "data": calldata}, "latest"],
        });
        let client = reqwest::Client::new();
        let resp: serde_json::Value = client
            .post(&ctx.rpc_url)
            .json(&body)
            .send().await
            .map_err(|e| anyhow::anyhow!("RPC POST failed: {e}"))?
            .json().await
            .map_err(|e| anyhow::anyhow!("RPC response was not JSON: {e}"))?;

        if let Some(err) = resp.get("error") {
            anyhow::bail!("oracle eth_call reverted (likely paused or unconfigured): {err}");
        }
        let result_hex = resp.get("result")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("RPC response missing `result` field: {resp}"))?;

        let bytes = hex::decode(result_hex.trim_start_matches("0x"))
            .map_err(|e| anyhow::anyhow!("result is not hex: {e}"))?;
        if bytes.len() < 8 * 32 {
            anyhow::bail!("tvlBreakdown() returned {} bytes, expected >= 256", bytes.len());
        }
        let read = |i: usize| U256::from_big_endian(&bytes[i * 32..(i + 1) * 32]);
        let amm       = read(0);
        let lending   = read(1);
        let stability = read(2);
        let staking   = read(3);
        let reward    = read(4);
        let bridge    = read(5);
        let total     = read(6);
        let timestamp = read(7).low_u64();

        if self.json {
            let out = serde_json::json!({
                "oracle": format!("0x{}", hex::encode(oracle)),
                "block_timestamp": timestamp,
                "amm_usd":         amm.to_string(),
                "lending_usd":     lending.to_string(),
                "stability_usd":   stability.to_string(),
                "staking_usd":     staking.to_string(),
                "reward_usd":      reward.to_string(),
                "bridge_vault_usd": bridge.to_string(),
                "total_usd":       total.to_string(),
                "decimals": 18,
            });
            println!("{}", serde_json::to_string_pretty(&out)?);
        } else {
            println!("Oracle:    0x{}", hex::encode(oracle));
            println!("Timestamp: {timestamp}");
            println!("RPC:       {}", ctx.rpc_url);
            println!();
            println!("Source         USD (raw, 18-decimal)");
            println!("────────────   ─────────────────────────────────────");
            println!("AMM            {amm}");
            println!("Lending        {lending}");
            println!("Stability      {stability}");
            println!("Staking        {staking}");
            println!("Reward         {reward}");
            println!("Bridge Vault   {bridge}");
            println!("────────────   ─────────────────────────────────────");
            println!("TOTAL          {total}");
        }
        Ok(())
    }
}

// ─── helpers ──────────────────────────────────────────────────────────────────

fn parse_addr(s: &str) -> anyhow::Result<[u8; 20]> {
    let h = s.strip_prefix("0x").unwrap_or(s);
    let raw = hex::decode(h).map_err(|e| anyhow::anyhow!("address is not hex: {e}"))?;
    if raw.len() != 20 { anyhow::bail!("address must be 20 bytes, got {}", raw.len()); }
    let mut out = [0u8; 20];
    out.copy_from_slice(&raw);
    Ok(out)
}

pub async fn run(cmd: DefiCmd, ctx: &Context) -> anyhow::Result<()> {
    match cmd.sub {
        DefiSub::TokenBalance(c)    => c.run(ctx).await,
        DefiSub::TokenTransfer(c)   => c.run(ctx).await,
        DefiSub::TokenApprove(c)    => c.run(ctx).await,
        DefiSub::SwapQuote(c)       => c.run(ctx).await,
        DefiSub::SwapExecute(c)     => c.run(ctx).await,
        DefiSub::AddLiquidity(c)    => c.run(ctx).await,
        DefiSub::RemoveLiquidity(c) => c.run(ctx).await,
        DefiSub::OraclePrice(c)     => c.run(ctx).await,
        DefiSub::Tvl(c)             => c.run(ctx).await,
    }
}
