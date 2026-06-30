//! Runtime configuration for the CLI.
//!
//! Wraps the parsed `Cli` into a richer `Context` that:
//! - Validates the RPC endpoint scheme (T214).
//! - Lazily loads the keystore JSON for cheap lookups (`signer_address`).
//! - Resolves the keystore password and unlocks the wallet on demand
//!   (`signer`), routing through `safety::resolve_password`.
//!
//! Network I/O lives elsewhere — Context is intentionally synchronous so
//! every command starts from the same trust posture before touching the
//! network.

use std::path::PathBuf;

use anyhow::Context as _;
use url::Url;
use zbx_keystore::{KeyFile, KeystoreWallet};
use zbx_types::Address;

use crate::safety;

/// All ambient policy a CLI subcommand needs to evaluate its inputs.
///
/// Built once in `main()` and passed by reference to every subcommand. The
/// `cli` field carries forward the parsed flags so the password and
/// confirmation helpers in `safety` can read them.
pub struct Context {
    /// Validated RPC endpoint URL (scheme is `http`/`https`/`ws`/`wss`).
    pub rpc_url: String,
    /// Signing chain ID.
    pub chain_id: u64,
    /// Optional path to an Ethereum v3 keystore JSON file.
    pub keystore_path: Option<PathBuf>,
    /// Re-exposed parsed CLI for downstream password / confirmation lookups.
    pub cli: CliPolicy,
}

/// The subset of CLI flags the safety helpers consult. Lives in this
/// module (rather than in `main.rs` next to `Cli`) so the library half of
/// this crate can build standalone — `Cli`/`Commands` are only defined in
/// the binary entry point and would otherwise leak `clap` derive coupling
/// across the lib boundary.
#[derive(Clone, Debug)]
pub struct CliPolicy {
    pub password_stdin: bool,
    pub password_file: Option<PathBuf>,
    pub allow_insecure_rpc: bool,
    pub yes: bool,
}

impl Context {
    /// Build a `Context` straight from already-validated input. Called by
    /// `main()` after `clap` parsing, and by tests that want to fabricate
    /// a context without going through argv.
    pub fn from_parts(
        rpc_url: String,
        chain_id: u64,
        keystore_path: Option<PathBuf>,
        policy: CliPolicy,
    ) -> anyhow::Result<Self> {
        validate_rpc_url(&rpc_url, policy.allow_insecure_rpc)
            .with_context(|| format!("rpc-url {rpc_url}"))?;
        Ok(Self { rpc_url, chain_id, keystore_path, cli: policy })
    }

    // ─── Cheap (no-unlock) lookups ─────────────────────────────────────────

    /// Resolve the address of the configured keystore without prompting for
    /// the password. Reads only the public `address` field of the v3 JSON.
    pub fn signer_address(&self) -> anyhow::Result<Address> {
        let kf = self.load_keyfile()?;
        let bytes = kf.address_bytes()
            .map_err(|e| anyhow::anyhow!("decode keystore address: {e}"))?;
        Ok(Address(bytes))
    }

    fn load_keyfile(&self) -> anyhow::Result<KeyFile> {
        let path = self.keystore_path.as_ref()
            .ok_or_else(|| anyhow::anyhow!(
                "no keystore configured — pass --keystore <path> or set ZBX_KEYSTORE"
            ))?;
        let raw = std::fs::read(path)
            .with_context(|| format!("read keystore {}", path.display()))?;
        KeyFile::from_json(&raw)
            .map_err(|e| anyhow::anyhow!("parse keystore {}: {e}", path.display()))
    }

    // ─── Sensitive (unlock) operations ─────────────────────────────────────

    /// Unlock the configured keystore. Resolves the password via
    /// `safety::resolve_password` (TTY / `--password-stdin` /
    /// `--password-file`) and decrypts the keyfile. Returns a wallet that
    /// can sign messages and EIP-1559 transactions.
    pub fn signer(&self) -> anyhow::Result<KeystoreWallet> {
        let kf = self.load_keyfile()?;
        let path_display = self.keystore_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<keystore>".into());

        let pw = safety::resolve_password(&self.cli, "Keystore password: ")?;

        // Distinct error on bad password (T209): callers can grep for
        // "InvalidPassword" to surface a non-zero, scriptable exit signal.
        KeystoreWallet::from_keyfile(&kf, &pw).map_err(|e| match e {
            zbx_keystore::KeystoreError::InvalidPassword =>
                anyhow::anyhow!(
                    "InvalidPassword: keystore {path_display} could not be decrypted with the supplied password"
                ),
            other => anyhow::anyhow!("unlock {path_display}: {other}"),
        })
    }

    // ─── Re-exports of safety helpers for ergonomic call sites ─────────────

    pub fn confirm_or_yes(&self, summary: &str) -> anyhow::Result<()> {
        safety::confirm_or_yes(&self.cli, summary)
    }

    pub fn resolve_password(&self, prompt: &str) -> anyhow::Result<String> {
        safety::resolve_password(&self.cli, prompt)
    }
}

/// Reject plain `http://` against non-localhost endpoints unless the user
/// has opted in via `--allow-insecure-rpc`.
fn validate_rpc_url(raw: &str, allow_insecure: bool) -> anyhow::Result<()> {
    let url = Url::parse(raw)
        .map_err(|e| anyhow::anyhow!("invalid URL: {e}"))?;
    let scheme = url.scheme();
    match scheme {
        "https" | "wss" => Ok(()),
        "http" | "ws" => {
            let host = url.host_str().unwrap_or("");
            let is_local = host == "localhost"
                || host == "127.0.0.1"
                || host == "::1"
                || host == "0.0.0.0"
                || host.ends_with(".local")
                || host.ends_with(".localhost");
            if is_local || allow_insecure {
                Ok(())
            } else {
                anyhow::bail!(
                    "refusing plain {scheme}:// against non-localhost host {host:?}; \
                     pass --allow-insecure-rpc to override (NOT recommended)"
                )
            }
        }
        other => anyhow::bail!("unsupported RPC scheme {other:?} (use https/http/wss/ws)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_always_allowed() {
        assert!(validate_rpc_url("https://rpc.zbx.network", false).is_ok());
    }

    #[test]
    fn http_localhost_allowed() {
        assert!(validate_rpc_url("http://localhost:8545", false).is_ok());
        assert!(validate_rpc_url("http://127.0.0.1:8545", false).is_ok());
    }

    #[test]
    fn http_remote_rejected_by_default() {
        let err = validate_rpc_url("http://rpc.example.com", false).unwrap_err();
        assert!(err.to_string().contains("refusing"), "msg = {err}");
    }

    #[test]
    fn http_remote_allowed_with_override() {
        assert!(validate_rpc_url("http://rpc.example.com", true).is_ok());
    }

    #[test]
    fn unknown_scheme_rejected() {
        assert!(validate_rpc_url("ftp://rpc", false).is_err());
    }
}
