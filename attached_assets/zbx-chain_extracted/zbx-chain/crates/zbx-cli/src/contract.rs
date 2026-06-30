//! Contract commands — deploy, call (read), send (write), decode.
//!
//! M53-04 FIX:
//!   - `call`   is now wired to `eth_call` (read-only, no signing needed).
//!   - `send`   is now wired to `eth_sendRawTransaction` (signs + broadcasts).
//!   - `deploy` is now wired to `eth_sendRawTransaction` (contract creation tx).
//!   - `decode` is a local ABI decode helper (unchanged — no RPC needed).

use clap::{Args, Subcommand};
use std::path::PathBuf;

use crate::config::Context;
use crate::rpc;

#[derive(Args, Debug)]
pub struct ContractCmd {
    #[command(subcommand)]
    pub sub: ContractSub,
}

#[derive(Subcommand, Debug)]
pub enum ContractSub {
    /// Deploy a compiled contract.
    Deploy(ContractDeploy),
    /// Read-only function call (no gas, no state change).
    Call(ContractCall),
    /// Write transaction to a contract function (signs + broadcasts).
    Send(ContractSend),
    /// Decode ABI-encoded calldata.
    Decode(ContractDecode),
}

// ── deploy ────────────────────────────────────────────────────────────────────

#[derive(Args, Debug)]
pub struct ContractDeploy {
    /// Path to compiled bytecode (.bin file or hex string).
    #[arg(long)] pub bytecode: PathBuf,
    /// Path to ABI JSON file (needed for constructor args).
    #[arg(long)] pub abi: Option<PathBuf>,
    /// Constructor arguments (comma-separated).
    #[arg(long, default_value = "")] pub args: String,
    /// Native ZBX value to send with deployment (wei).
    #[arg(long, default_value = "0")] pub value: u128,
    /// Gas limit override. Default 2,000,000.
    #[arg(long)] pub gas: Option<u64>,
}

impl ContractDeploy {
    pub async fn run(&self, ctx: &Context) -> anyhow::Result<()> {
        let from = ctx.signer_address()?;
        let bytecode_raw = std::fs::read_to_string(&self.bytecode)
            .map_err(|e| anyhow::anyhow!("read {}: {e}", self.bytecode.display()))?;
        let bytecode_hex = bytecode_raw.trim().strip_prefix("0x")
            .unwrap_or(bytecode_raw.trim());
        let bytecode = hex::decode(bytecode_hex)
            .map_err(|e| anyhow::anyhow!("bytecode is not hex: {e}"))?;
        let gas = self.gas.unwrap_or(2_000_000u64);

        let summary = format!(
            "=== Contract Deploy preflight ===\n\
             from        : 0x{from_hex}\n  chain id    : {chain}\n\
             rpc         : {rpc}\n  bytecode    : {} bytes\n\
             abi         : {abi}\n  args        : {args:?}\n\
             value (wei) : {value}\n  gas         : {gas}",
            bytecode.len(),
            from_hex = hex::encode(from.0),
            chain    = ctx.chain_id,
            rpc      = ctx.rpc_url,
            abi      = self.abi.as_ref().map(|p| p.display().to_string())
                           .unwrap_or_else(|| "<none>".into()),
            args     = self.args,
            value    = self.value,
        );
        ctx.confirm_or_yes(&summary)?;
        let wallet = ctx.signer()?;

        let tx_hash = rpc::build_and_send_tx(
            &ctx.rpc_url, &wallet, ctx.chain_id,
            None, &bytecode, self.value, gas,
        ).await?;
        println!("Deploy submitted. tx: {tx_hash}");
        println!("(Use eth_getTransactionReceipt to get the contract address)");
        Ok(())
    }
}

// ── call (read) ───────────────────────────────────────────────────────────────

#[derive(Args, Debug)]
pub struct ContractCall {
    #[arg(long)] pub address: String,
    #[arg(long)] pub abi: PathBuf,
    /// Function signature e.g. "balanceOf(address)".
    #[arg(long, short = 'f')] pub func: String,
    #[arg(long, default_value = "")] pub args: String,
    #[arg(long)] pub from: Option<String>,
}

impl ContractCall {
    pub async fn run(&self, ctx: &Context) -> anyhow::Result<()> {
        let target = parse_addr(&self.address)?;
        if !self.abi.exists() {
            anyhow::bail!("ABI file not found: {}", self.abi.display());
        }

        // Build 4-byte selector from the function signature.
        let selector = rpc::selector(&self.func);

        // For now we support zero-argument calls (cover the most common read
        // queries: totalSupply(), name(), symbol(), decimals(), owner(), etc.).
        // ABI argument encoding for calls with args requires an ABI parser
        // (out of scope for this CLI — use cast(1) from Foundry for complex calls).
        let args_raw = self.args.trim();
        if !args_raw.is_empty() {
            anyhow::bail!(
                "argument encoding is not supported in this build — pass --args '' \
                 for zero-argument read calls, or use `cast call` (Foundry) for complex ABI calls"
            );
        }

        eprintln!(
            "=== eth_call ===\n  contract : {}\n  fn       : {}\n  selector : 0x{}\n  rpc      : {}",
            self.address, self.func, hex::encode(selector), ctx.rpc_url,
        );

        let raw = rpc::eth_call_raw(&ctx.rpc_url, &target, &selector).await?;

        // Print raw hex result — callers can pipe through `cast abi-decode` or similar.
        println!("result (hex): 0x{}", hex::encode(&raw));

        // Attempt to decode as uint256 for convenience.
        if raw.len() == 32 {
            let as_u128 = u128::from_be_bytes(raw[16..32].try_into().unwrap());
            println!("result (uint256 low 128): {as_u128}");
        }
        // Attempt to decode as UTF-8 string (for name/symbol).
        if raw.len() >= 64 {
            let offset = u64::from_be_bytes(raw[24..32].try_into().unwrap()) as usize;
            let end = (offset + 32).min(raw.len());
            if end <= raw.len() {
                let len = u64::from_be_bytes(raw[offset+24..offset+32].try_into()
                    .unwrap_or([0u8;8])) as usize;
                let str_start = offset + 32;
                let str_end = (str_start + len).min(raw.len());
                if str_start < str_end {
                    if let Ok(s) = std::str::from_utf8(&raw[str_start..str_end]) {
                        println!("result (string): {s:?}");
                    }
                }
            }
        }
        Ok(())
    }
}

// ── send (write) ──────────────────────────────────────────────────────────────

#[derive(Args, Debug)]
pub struct ContractSend {
    #[arg(long)] pub address: String,
    #[arg(long)] pub abi: PathBuf,
    #[arg(long, short = 'f')] pub func: String,
    #[arg(long, default_value = "")] pub args: String,
    #[arg(long, default_value = "0")] pub value: u128,
    #[arg(long)] pub gas: Option<u64>,
    #[arg(long)] pub max_fee_per_gas: Option<u64>,
    #[arg(long)] pub max_priority_fee: Option<u64>,
}

impl ContractSend {
    pub async fn run(&self, ctx: &Context) -> anyhow::Result<()> {
        let target = parse_addr(&self.address)?;
        let from   = ctx.signer_address()?;
        if !self.abi.exists() {
            anyhow::bail!("ABI file not found: {}", self.abi.display());
        }

        let args_raw = self.args.trim();
        if !args_raw.is_empty() {
            anyhow::bail!(
                "argument encoding is not supported in this build for `contract send` — \
                 pass --args '' for zero-argument calls, or use `cast send` (Foundry) for \
                 complex ABI calls with arguments"
            );
        }

        let selector = rpc::selector(&self.func);
        let gas      = self.gas.unwrap_or(300_000u64);

        let summary = format!(
            "=== Contract Send preflight ===\n\
             from        : 0x{from_hex}\n  to          : 0x{to_hex}\n\
             chain id    : {chain}\n  rpc         : {rpc}\n\
             function    : {func}  (selector: 0x{sel})\n\
             value (wei) : {value}\n  gas         : {gas}",
            from_hex = hex::encode(from.0),
            to_hex   = hex::encode(target),
            chain    = ctx.chain_id,
            rpc      = ctx.rpc_url,
            func     = self.func,
            sel      = hex::encode(selector),
            value    = self.value,
        );
        ctx.confirm_or_yes(&summary)?;
        let wallet = ctx.signer()?;

        let tx_hash = rpc::build_and_send_tx(
            &ctx.rpc_url, &wallet, ctx.chain_id,
            Some(&target), &selector, self.value, gas,
        ).await?;
        println!("Send submitted. tx: {tx_hash}");
        Ok(())
    }
}

// ── decode ────────────────────────────────────────────────────────────────────

#[derive(Args, Debug)]
pub struct ContractDecode {
    #[arg(long)] pub abi: PathBuf,
    #[arg(long)] pub data: String,
}

impl ContractDecode {
    pub async fn run(&self, _ctx: &Context) -> anyhow::Result<()> {
        if !self.abi.exists() {
            anyhow::bail!("ABI file not found: {}", self.abi.display());
        }
        let hex_str = self.data.strip_prefix("0x").unwrap_or(&self.data);
        let bytes = hex::decode(hex_str)
            .map_err(|e| anyhow::anyhow!("calldata is not hex: {e}"))?;

        if bytes.len() < 4 {
            anyhow::bail!("calldata too short (< 4 bytes) to contain a selector");
        }
        let sel = hex::encode(&bytes[..4]);
        println!("Selector : 0x{sel}");
        println!("Args hex : 0x{}", hex::encode(&bytes[4..]));
        println!("Total    : {} bytes", bytes.len());
        println!("(Full ABI decode requires the ABI parser — pipe to `cast decode-calldata` for named decoding)");
        Ok(())
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn parse_addr(s: &str) -> anyhow::Result<[u8; 20]> {
    let h = s.strip_prefix("0x").unwrap_or(s);
    let raw = hex::decode(h).map_err(|e| anyhow::anyhow!("address is not hex: {e}"))?;
    if raw.len() != 20 { anyhow::bail!("address must be 20 bytes, got {}", raw.len()); }
    let mut out = [0u8; 20];
    out.copy_from_slice(&raw);
    Ok(out)
}

pub async fn run(cmd: ContractCmd, ctx: &Context) -> anyhow::Result<()> {
    match cmd.sub {
        ContractSub::Deploy(c) => c.run(ctx).await,
        ContractSub::Call(c)   => c.run(ctx).await,
        ContractSub::Send(c)   => c.run(ctx).await,
        ContractSub::Decode(c) => c.run(ctx).await,
    }
}
