//! Wallet commands — key generation, import, export, signing.
//!
//! Talks **directly** to `zbx-keystore` — there is no SDK abstraction in
//! between, so the audit trail from the CLI down to the AES/scrypt
//! primitives is one short hop.
//!
//! ## Examples
//! ```bash
//! zbxctl wallet new --save ./my.keystore
//! zbxctl wallet import --save ./my.keystore   # then paste hex on stdin
//! zbxctl wallet address --keystore ./my.keystore
//! zbxctl wallet sign    --message "hello zbx" --keystore ./my.keystore
//! zbxctl wallet export  --keystore ./my.keystore --unsafe-show-private-key
//! ```

use clap::{Args, Subcommand};
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::PathBuf;

use zbx_keystore::KeystoreWallet;
use zbx_types::H256;
use sha3::{Digest, Keccak256};

use crate::config::Context;
use crate::safety;

// ─── Top-level ──────────────────────────────────────────────────────────────

#[derive(Args, Debug)]
pub struct WalletCmd {
    #[command(subcommand)]
    pub sub: WalletSub,
}

#[derive(Subcommand, Debug)]
pub enum WalletSub {
    /// Generate a new random wallet.
    New(WalletNew),
    /// Import a wallet from a raw private key (hex on STDIN).
    Import(WalletImport),
    /// Show the address for the currently configured keystore.
    Address(WalletAddress),
    /// Sign a message (EIP-191 personal_sign) with the current wallet.
    Sign(WalletSign),
    /// Export the private key from a keystore (gated, requires confirmation).
    Export(WalletExport),
}

// ── wallet new ─────────────────────────────────────────────────────────────

#[derive(Args, Debug)]
pub struct WalletNew {
    /// Save the encrypted keystore to this path.
    #[arg(long)]
    pub save: Option<PathBuf>,

    /// scrypt cost parameter `N` (must be a power of two). Defaults to the
    /// Ethereum mainnet strength of 262144. Lower values (e.g. 8192) speed
    /// the prompt up at the cost of brute-force resistance.
    #[arg(long, default_value = "262144")]
    pub scrypt_n: u32,
}

impl WalletNew {
    pub async fn run(&self, ctx: &Context) -> anyhow::Result<()> {
        let wallet = KeystoreWallet::from_random()
            .map_err(|e| anyhow::anyhow!("generate wallet: {e}"))?;
        let addr_hex = format!("0x{}", hex::encode(wallet.address()));

        println!("Address  : {addr_hex}");
        println!("Chain ID : {} (ZBX)", ctx.chain_id);

        if let Some(path) = &self.save {
            let summary = format!(
                "About to create a NEW keystore file:\n  path  : {}\n  addr  : {addr_hex}\n  KDF   : scrypt N={}",
                path.display(), self.scrypt_n,
            );
            ctx.confirm_or_yes(&summary)?;
            let pw = ctx.resolve_password("New keystore password: ")?;
            let pw2 = ctx.resolve_password("Confirm password    : ")?;
            if pw != pw2 {
                anyhow::bail!("passwords did not match — keystore not written");
            }
            let kf = wallet.to_keyfile(&pw, self.scrypt_n)
                .map_err(|e| anyhow::anyhow!("encrypt keystore: {e}"))?;
            let json = kf.to_json()
                .map_err(|e| anyhow::anyhow!("serialize keystore: {e}"))?;
            write_keystore(path, &json)?;
            println!("Keystore : saved to {}", path.display());
        } else {
            println!("(pass --save <path> to persist this wallet to disk)");
        }
        Ok(())
    }
}

// ── wallet import ───────────────────────────────────────────────────────────

#[derive(Args, Debug)]
pub struct WalletImport {
    /// Save the imported wallet as an encrypted keystore at this path.
    #[arg(long)]
    pub save: Option<PathBuf>,

    /// scrypt cost parameter (see `wallet new`).
    #[arg(long, default_value = "262144")]
    pub scrypt_n: u32,
}

impl WalletImport {
    pub async fn run(&self, ctx: &Context) -> anyhow::Result<()> {
        // The private key is read from STDIN to keep it out of `argv`/`ps`.
        // No `--private-key` value flag is provided (T210).
        if io::stdin().is_terminal() {
            eprintln!(
                "Paste the private key (hex, with or without '0x' prefix) on STDIN, \
                 then press <Enter><Ctrl-D>. Input is not echoed-suppressed; if \
                 you are on a shared terminal, abort and use a file pipe instead."
            );
        }
        let mut line = String::new();
        io::stdin().lock().read_line(&mut line)
            .map_err(|e| anyhow::anyhow!("read private key: {e}"))?;
        let trimmed = line.trim();
        let hex_no_prefix = trimmed.strip_prefix("0x").unwrap_or(trimmed);
        let raw = hex::decode(hex_no_prefix)
            .map_err(|e| anyhow::anyhow!("invalid hex private key: {e}"))?;
        if raw.len() != 32 {
            anyhow::bail!("private key must be exactly 32 bytes, got {}", raw.len());
        }
        let mut sk = [0u8; 32];
        sk.copy_from_slice(&raw);

        let wallet = KeystoreWallet::from_private_key(&sk)
            .map_err(|e| anyhow::anyhow!("import private key: {e}"))?;
        // Wipe the local stack copy of the secret immediately.
        for b in sk.iter_mut() { *b = 0; }

        let addr_hex = format!("0x{}", hex::encode(wallet.address()));
        println!("Imported address : {addr_hex}");

        if let Some(path) = &self.save {
            let summary = format!(
                "About to write an IMPORTED keystore:\n  path  : {}\n  addr  : {addr_hex}",
                path.display(),
            );
            ctx.confirm_or_yes(&summary)?;
            let pw = ctx.resolve_password("New keystore password: ")?;
            let pw2 = ctx.resolve_password("Confirm password    : ")?;
            if pw != pw2 {
                anyhow::bail!("passwords did not match — keystore not written");
            }
            let kf = wallet.to_keyfile(&pw, self.scrypt_n)
                .map_err(|e| anyhow::anyhow!("encrypt keystore: {e}"))?;
            let json = kf.to_json()
                .map_err(|e| anyhow::anyhow!("serialize keystore: {e}"))?;
            write_keystore(path, &json)?;
            println!("Keystore         : saved to {}", path.display());
        }
        Ok(())
    }
}

// ── wallet address ──────────────────────────────────────────────────────────

#[derive(Args, Debug)]
pub struct WalletAddress {
    /// Override the global `--keystore` for this lookup only.
    #[arg(long)]
    pub keystore: Option<PathBuf>,
}

impl WalletAddress {
    pub async fn run(&self, ctx: &Context) -> anyhow::Result<()> {
        let addr = if let Some(path) = &self.keystore {
            // Read the override path directly without touching ctx state.
            let raw = std::fs::read(path)
                .map_err(|e| anyhow::anyhow!("read {}: {e}", path.display()))?;
            let kf = zbx_keystore::KeyFile::from_json(&raw)
                .map_err(|e| anyhow::anyhow!("parse {}: {e}", path.display()))?;
            let bytes = kf.address_bytes()
                .map_err(|e| anyhow::anyhow!("decode address: {e}"))?;
            zbx_types::Address(bytes)
        } else {
            ctx.signer_address()?
        };
        println!("0x{}", hex::encode(addr.0));
        Ok(())
    }
}

// ── wallet sign ─────────────────────────────────────────────────────────────

#[derive(Args, Debug)]
pub struct WalletSign {
    /// Message to sign (EIP-191 personal_sign format).
    #[arg(long)]
    pub message: String,
}

impl WalletSign {
    pub async fn run(&self, ctx: &Context) -> anyhow::Result<()> {
        // SEC-2026-05-09 (N3): require explicit confirmation before producing
        // a signature. `wallet sign` is a privilege escalation primitive —
        // an attacker with shell access could otherwise trick the CLI into
        // signing arbitrary EIP-191 messages (e.g. permit() approvals,
        // off-chain auth challenges) without the operator noticing.
        let summary = format!(
            "About to EIP-191 personal_sign the following message with the \
             current wallet:\n  ─── BEGIN MESSAGE ───\n  {}\n  ─── END MESSAGE ───\n\
             Signed messages can authorise off-chain actions (login, permits, \
             trades) — only proceed if you typed this command yourself.",
            self.message,
        );
        ctx.confirm_or_yes(&summary)?;

        let wallet = ctx.signer()?;
        let msg = self.message.as_bytes();
        // EIP-191 personal_sign prefix: "\x19Ethereum Signed Message:\n<len>"
        let mut hasher = Keccak256::new();
        let prefix = format!("\x19Ethereum Signed Message:\n{}", msg.len());
        hasher.update(prefix.as_bytes());
        hasher.update(msg);
        let digest = hasher.finalize();
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&digest);
        let hash = H256(hash);

        let sig = wallet.sign(&hash)
            .map_err(|e| anyhow::anyhow!("sign: {e}"))?;
        println!("Address    : 0x{}", hex::encode(wallet.address()));
        println!("Signature  : 0x{}", hex::encode(sig.to_bytes()));
        Ok(())
    }
}

// ── wallet export (gated) ───────────────────────────────────────────────────

#[derive(Args, Debug)]
pub struct WalletExport {
    /// Keystore file to decrypt and export. Falls back to the global
    /// `--keystore` flag when omitted.
    #[arg(long)]
    pub keystore: Option<PathBuf>,

    /// Acknowledge that the next line of STDOUT will contain the raw
    /// private key. Without this flag the command refuses to run, even on
    /// a TTY, even with `--yes` (T210).
    #[arg(long)]
    pub unsafe_show_private_key: bool,
}

impl WalletExport {
    pub async fn run(&self, ctx: &Context) -> anyhow::Result<()> {
        if !self.unsafe_show_private_key {
            anyhow::bail!(
                "wallet export refused: pass --unsafe-show-private-key to acknowledge \
                 that the next STDOUT line will contain the raw secret"
            );
        }

        // Choose the keystore path: per-command override > global.
        let path: PathBuf = self.keystore
            .clone()
            .or_else(|| ctx.keystore_path.clone())
            .ok_or_else(|| anyhow::anyhow!(
                "no keystore configured — pass --keystore <path> or set ZBX_KEYSTORE"
            ))?;

        let summary = format!(
            "================ DANGEROUS: PRIVATE KEY EXPORT ================\n\
             About to PRINT THE PRIVATE KEY for keystore:\n  {}\n\
             Anyone who reads this terminal, scrollback, log, or shoulder-surfs \
             will see the secret.\n\
             NOTE: --yes is intentionally NOT honored for this action; you \
             must type the full word 'yes' on a TTY.\n\
             ===============================================================",
            path.display(),
        );
        // T210: strict-mode confirmation — ignores --yes, requires TTY,
        // requires literal "yes" (not "y"/"YES"/etc).
        safety::confirm_strict(&summary)?;

        let raw = std::fs::read(&path)
            .map_err(|e| anyhow::anyhow!("read {}: {e}", path.display()))?;
        let kf = zbx_keystore::KeyFile::from_json(&raw)
            .map_err(|e| anyhow::anyhow!("parse {}: {e}", path.display()))?;
        let pw = ctx.resolve_password("Keystore password: ")?;
        let wallet = KeystoreWallet::from_keyfile(&kf, &pw)
            .map_err(|e| match e {
                zbx_keystore::KeystoreError::InvalidPassword =>
                    anyhow::anyhow!("InvalidPassword"),
                other => anyhow::anyhow!("unlock: {other}"),
            })?;

        println!("Address     : 0x{}", hex::encode(wallet.address()));
        println!("Private key : 0x{}", hex::encode(wallet.expose_private_key_unsafe()));
        eprintln!();
        eprintln!("WARNING: clear your scrollback (`clear; reset`) before leaving this terminal.");
        Ok(())
    }
}

// ── filesystem helper ───────────────────────────────────────────────────────

/// Write a keystore JSON file with restrictive permissions (0600 on Unix).
/// Refuses to overwrite an existing file.
///
/// Task #12: previously this function rolled its own `OpenOptions::mode(0o600)`
/// dance. Now delegates to `zbx_keystore::secure_write` so every keystore
/// write site in the workspace shares one audited code path. The pre-flight
/// `path.exists()` check is kept so we surface a friendly, CLI-shaped error
/// message instead of `KeystoreError::Io(AlreadyExists)`.
fn write_keystore(path: &PathBuf, json: &[u8]) -> anyhow::Result<()> {
    if path.exists() {
        anyhow::bail!(
            "refusing to overwrite existing file {} — pick a fresh path",
            path.display()
        );
    }
    zbx_keystore::secure_write(path, json)
        .map_err(|e| anyhow::anyhow!("write keystore {}: {e}", path.display()))
}

pub async fn run(cmd: WalletCmd, ctx: &Context) -> anyhow::Result<()> {
    match cmd.sub {
        WalletSub::New(c)     => c.run(ctx).await,
        WalletSub::Import(c)  => c.run(ctx).await,
        WalletSub::Address(c) => c.run(ctx).await,
        WalletSub::Sign(c)    => c.run(ctx).await,
        WalletSub::Export(c)  => c.run(ctx).await,
    }
}
