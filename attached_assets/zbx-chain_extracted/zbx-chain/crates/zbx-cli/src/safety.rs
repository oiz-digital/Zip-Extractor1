//! Safety rails shared by every CLI subcommand.
//!
//! ## What lives here
//! - **Password resolution** (T210): TTY prompt OR `--password-stdin` OR
//!   `--password-file`. Plaintext `--password=...` flags are deliberately
//!   absent so secrets never appear in `ps aux` or shell history.
//! - **Confirmation prompts** (T211): every destructive subcommand prints a
//!   preflight summary and waits for `[y/N]` unless `--yes` was passed.
//! - **Slippage clamp** (T212): bps must lie in `[1, 1_000]`.
//! - **Selector warnings** (T213): well-known dangerous selectors print a
//!   loud warning before the user is asked to confirm.
//!
//! Every helper here returns `anyhow::Error` so command code can `?` it
//! straight through.

use std::io::{self, BufRead, IsTerminal, Write};
use std::path::Path;

use crate::config::CliPolicy;

// ─── Password resolution (T210) ─────────────────────────────────────────────

/// Resolve a keystore password for an action that needs to unlock a key.
///
/// Resolution order (any single source must produce a non-empty password):
/// 1. `--password-stdin` flag → read one line from STDIN.
/// 2. `--password-file <path>` flag → read the first line of the file.
/// 3. Interactive TTY prompt via `rpassword`.
///
/// We deliberately do **not** support a `--password=...` value flag: any
/// argument visible in `argv` ends up in `ps aux`, shell history, audit
/// logs, and process accounting.
pub fn resolve_password(cli: &CliPolicy, prompt: &str) -> anyhow::Result<String> {
    if cli.password_stdin {
        let stdin = io::stdin();
        let mut line = String::new();
        stdin.lock().read_line(&mut line)
            .map_err(|e| anyhow::anyhow!("read --password-stdin: {e}"))?;
        // Strip exactly one trailing CR/LF pair so a literal trailing newline
        // in the user's password is preserved if they really want it (they
        // can avoid `read_line` by piping via password-file instead).
        let trimmed = line
            .strip_suffix('\n').unwrap_or(&line)
            .strip_suffix('\r').unwrap_or(line.strip_suffix('\n').unwrap_or(&line))
            .to_string();
        if trimmed.is_empty() {
            anyhow::bail!("--password-stdin produced an empty password");
        }
        return Ok(trimmed);
    }

    if let Some(path) = &cli.password_file {
        return read_password_file(path);
    }

    // Interactive TTY fallback. Refuse to prompt when STDIN isn't a terminal —
    // otherwise scripted callers silently hang.
    if !io::stdin().is_terminal() {
        anyhow::bail!(
            "no password source configured and STDIN is not a TTY; \
             pass --password-stdin or --password-file <path>"
        );
    }
    let pw = rpassword::prompt_password(prompt)
        .map_err(|e| anyhow::anyhow!("password prompt: {e}"))?;
    if pw.is_empty() {
        anyhow::bail!("empty password rejected");
    }
    Ok(pw)
}

fn read_password_file(path: &Path) -> anyhow::Result<String> {
    // We do a best-effort permissions check on Unix: refuse files that are
    // group- or world-readable. Other users on a multi-tenant box should not
    // be able to lift the password by reading the file.
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let md = std::fs::metadata(path)
            .map_err(|e| anyhow::anyhow!("stat {}: {e}", path.display()))?;
        let mode = md.mode() & 0o777;
        if mode & 0o077 != 0 {
            anyhow::bail!(
                "password file {} has unsafe permissions {:o} \
                 (must be 0400 or 0600)",
                path.display(), mode
            );
        }
    }

    let raw = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("read {}: {e}", path.display()))?;
    // Take the first line only; ignore trailing whitespace / blank lines.
    let pw = raw.lines().next().unwrap_or("").trim_end_matches('\r').to_string();
    if pw.is_empty() {
        anyhow::bail!("password file {} produced an empty password", path.display());
    }
    Ok(pw)
}

// ─── Confirmation prompts (T211) ────────────────────────────────────────────

/// Print `summary` to STDERR, then ask the user to confirm. Returns `Ok(())`
/// on `y`/`yes`. With `--yes` the prompt is skipped (the summary is still
/// printed so unattended runs leave a record of what was attempted).
pub fn confirm_or_yes(cli: &CliPolicy, summary: &str) -> anyhow::Result<()> {
    eprintln!("{summary}");

    if cli.yes {
        eprintln!("[--yes] confirmation skipped.");
        return Ok(());
    }

    if !io::stdin().is_terminal() {
        anyhow::bail!(
            "this action requires confirmation but STDIN is not a TTY; \
             re-run with --yes to acknowledge the preflight summary above"
        );
    }

    eprint!("Proceed? [y/N]: ");
    io::stderr().flush().ok();
    let mut answer = String::new();
    io::stdin().lock().read_line(&mut answer)
        .map_err(|e| anyhow::anyhow!("read confirmation: {e}"))?;
    let answer = answer.trim().to_lowercase();
    if answer == "y" || answer == "yes" {
        Ok(())
    } else {
        anyhow::bail!("aborted by user");
    }
}

/// Strict confirmation that ignores `--yes`. Used for irreversible /
/// secret-leaking actions (e.g. `wallet export`) where unattended approval
/// is itself a vulnerability — a CI job that ran `--yes` for everything
/// else must NOT also auto-confirm exporting a private key.
///
/// Requires:
///   - STDIN is a TTY (no piping a "yes\n" through stdin to fake it),
///   - the user types the literal word "yes" (not "y", not "YES").
pub fn confirm_strict(summary: &str) -> anyhow::Result<()> {
    eprintln!("{summary}");
    if !io::stdin().is_terminal() {
        anyhow::bail!(
            "this action requires interactive confirmation but STDIN is \
             not a TTY; --yes is intentionally not honored here"
        );
    }
    eprint!("Type 'yes' (lowercase, full word) to proceed: ");
    io::stderr().flush().ok();
    let mut answer = String::new();
    io::stdin().lock().read_line(&mut answer)
        .map_err(|e| anyhow::anyhow!("read confirmation: {e}"))?;
    if answer.trim() == "yes" {
        Ok(())
    } else {
        anyhow::bail!("aborted by user (did not type 'yes')");
    }
}

// ─── Slippage clamp (T212) ──────────────────────────────────────────────────

/// Bounds slippage tolerance (in basis points) to a sensible range.
///
/// - Minimum 1 bps (0.01%): anything lower means the swap can never fill
///   in practice.
/// - Maximum 1000 bps (10%): higher values are almost always a typo
///   ("100%" instead of "1%") that would let a sandwicher take 50% of the
///   notional. Real users override with a higher value extremely rarely;
///   we'd rather refuse than silently let the trade get drained.
pub fn slippage_clamp(bps: u32) -> anyhow::Result<u32> {
    const MIN_BPS: u32 = 1;
    const MAX_BPS: u32 = 1_000;
    if bps < MIN_BPS || bps > MAX_BPS {
        anyhow::bail!(
            "slippage {bps} bps is out of range [{MIN_BPS}, {MAX_BPS}]; \
             1 bps = 0.01%, 1000 bps = 10%"
        );
    }
    Ok(bps)
}

// ─── Selector warnings (T213) ───────────────────────────────────────────────

/// Inspect the first 4 bytes of an ABI-encoded calldata blob and emit a
/// loud warning if it matches a known dangerous governance selector.
/// Returns `Ok(Some(label))` if a warning was printed, `Ok(None)` otherwise.
pub fn decode_selector_warn(calldata_hex: &str) -> anyhow::Result<Option<&'static str>> {
    // Trim "0x" prefix if present and short-circuit for the explicit
    // empty / no-op case so the helper is safe to call unconditionally.
    let hex = calldata_hex.strip_prefix("0x").unwrap_or(calldata_hex);
    if hex.is_empty() || hex == "00" {
        return Ok(None);
    }
    if hex.len() < 8 {
        anyhow::bail!(
            "calldata too short ({} hex chars) to contain a 4-byte selector",
            hex.len()
        );
    }

    let selector = hex[..8].to_lowercase();

    // keccak256("setGovernor(address)")[..4] = 0xc42cf535
    // keccak256("upgradeTo(address)")[..4]  = 0x3659cfe6
    // keccak256("upgradeToAndCall(address,bytes)")[..4] = 0x4f1ef286
    // keccak256("transferOwnership(address)")[..4] = 0xf2fde38b
    // keccak256("renounceOwnership()")[..4] = 0x715018a6
    // keccak256("grantRole(bytes32,address)")[..4] = 0x2f2ff15d
    // keccak256("revokeRole(bytes32,address)")[..4] = 0xd547741f
    // keccak256("setAdmin(address)")[..4] = 0x704b6c02
    let label = match selector.as_str() {
        "c42cf535" => Some("setGovernor(address) — TRANSFERS GOVERNANCE CONTROL"),
        "3659cfe6" => Some("upgradeTo(address) — REPLACES IMPLEMENTATION CONTRACT"),
        "4f1ef286" => Some("upgradeToAndCall(address,bytes) — UPGRADES + REINITIALIZES"),
        "f2fde38b" => Some("transferOwnership(address) — TRANSFERS OWNERSHIP"),
        "715018a6" => Some("renounceOwnership() — PERMANENTLY REMOVES OWNER"),
        "2f2ff15d" => Some("grantRole(bytes32,address) — GRANTS A PRIVILEGED ROLE"),
        "d547741f" => Some("revokeRole(bytes32,address) — REVOKES A PRIVILEGED ROLE"),
        "704b6c02" => Some("setAdmin(address) — REASSIGNS THE ADMIN SLOT"),
        _          => None,
    };

    if let Some(text) = label {
        eprintln!("================================================================");
        eprintln!("WARNING — DANGEROUS SELECTOR DETECTED: 0x{selector}");
        eprintln!("  {text}");
        eprintln!("This calldata, if executed, will alter the privilege graph of the");
        eprintln!("target contract. Re-read the proposal description carefully and");
        eprintln!("verify the target address before confirming.");
        eprintln!("================================================================");
    }
    Ok(label)
}
