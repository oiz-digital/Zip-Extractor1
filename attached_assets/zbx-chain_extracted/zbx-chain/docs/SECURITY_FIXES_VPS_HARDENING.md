# SECURITY_FIXES — VPS keystore hardening (Task #12, 2026-05-12)

## Class of bug closed

Keystore files (validator BLS keys, ECDSA wallet keys, P2P static
keys) written with the process `umask` rather than an explicit
mode-0600 — typically world-readable on a poorly-configured VPS.
Pass-4 set the P2P static key correctly *after* writing it, but the
window between `fs::write` and `set_permissions` was a real
data-leak race on a multi-tenant VPS.

This pass:

1. Adds a single shared helper `zbx_keystore::secure_write` that
   creates the file with `OpenOptions::create_new(true).mode(0o600)`
   — atomic, no umask window, refuses to overwrite, fsyncs.
2. Adds `zbx_keystore::ensure_strict_perms` and
   `zbx_keystore::tighten_dir` so we can repair existing on-disk
   keyfiles whose permissions are loose.
3. Migrates every workspace keystore-write site to the helper.
4. Wires a startup scan into `zbx-node` that walks `<data_dir>` (and
   `<data_dir>/keys/`) and tightens any loose file with a `warn!`
   pointing operators at the leak.

## Audited keystore-write sites

Source-tree-wide grep on 2026-05-12. Anything writing secret material
to disk lives below:

| Site | Bytes written | Pre-fix mode | Post-fix mode | Notes |
|------|---------------|--------------|---------------|-------|
| `zbx-cli/src/wallet.rs::write_keystore` | Ethereum v3 keystore JSON (encrypted) | 0o600 (correct: pre-existing OpenOptions) | 0o600 (via `secure_write`) | Now consolidated through one code path. |
| `node/src/noise.rs::NoiseStaticKey::load_or_create` | 32-byte X25519 static private key | 0o600 (after a write→chmod race window) | 0o600 (atomic via `secure_write`) | Pass-4 originally shipped the chmod-after-write pattern; Task #12 closes the race. |
| `zbx-keystore/src/manager.rs` | (no writes — read-only manager) | n/a | n/a | Audited; see source. |
| `zbx-staking/src/**` | (no on-disk private-key writes — pubkeys + evidence only) | n/a | n/a | BLS private keys never touch disk in this crate; `VALIDATOR_KEY` is read from env per `node/src/main.rs`. |
| `node/src/bin/zbx-keygen.rs` | (no writes — prints private keys to stdout) | n/a | n/a | Operators are expected to redirect or paste manually; documented in the binary's `--help`. |

The rows for `zbx-staking` and `zbx-keygen` were re-confirmed during
this pass — neither persists private-key bytes to disk in the
current workspace, so no migration was required.

## Defence in depth

* `secure_write` refuses to overwrite an existing path. Callers that
  intend to rotate a key must explicitly `remove_file` first — this
  prevents an attacker who can dump bytes into `<data_dir>` from
  silently replacing a live keyfile.
* `secure_write` is `fsync`'d so a crash mid-write cannot leave a
  torn keystore on disk.
* `ensure_strict_perms` only ever tightens; it never loosens. A file
  that is already `0o400` (read-only for owner) is left untouched.
* `tighten_dir` is non-recursive by design — a deep walk could chase
  symlinks out of the data directory and tighten unrelated user
  files. Operators with deep key layouts must call `tighten_dir`
  explicitly per subdirectory.
* On Windows the helper falls back to default ACLs and emits a
  `warn!`. Production VPS deployments are Linux-first; Windows-based
  dev boxes are not expected to need VPS-grade ACLs and should layer
  NTFS directory ACLs at the parent dir.

## Tests

`cargo test -p zbx-keystore --lib` (Unix-only assertions are
`#[cfg(all(test, unix))]`):

* `secure_write_creates_with_0600`
* `secure_write_refuses_existing_path`
* `secure_write_creates_parent_dir`
* `ensure_strict_perms_tightens_loose_file`
* `ensure_strict_perms_noop_on_strict_file`
* `tighten_dir_handles_mixed_modes`
* `tighten_dir_missing_dir_is_noop`

## Honest gaps deferred

* **Encrypted-at-rest keystores** are out of scope (already covered
  by the PBKDF2-100k-iter wrap from Pass-4 on the v3 JSON
  ciphertext).
* **Hardware-wallet integration** is out of scope.
* **Keystore rotation policies** (TTL, audit log, dual-control) are
  out of scope.
* **Symlink hardening** — ✅ closed by Task #17 (2026-05-12).
  `secure_write` now opens with `O_NOFOLLOW` on Unix (and
  `FILE_FLAG_OPEN_REPARSE_POINT` on Windows) so a planted symlink at
  the keystore path cannot redirect the write to an attacker-chosen
  target — the open fails with `ELOOP` / `FilesystemLoop` and the
  link is left in place. Operators who deliberately stage a keystore
  on a network mount or other indirection have an explicit opt-in
  helper, `secure_write_follow_symlinks`, which manually resolves the
  link before applying `create_new` (so the swap-attack window
  cannot be re-introduced silently) and emits a `warn!` on every
  call so the fact that a secret path is being written through a
  symlink shows up in the deployment audit log. New unit tests
  `secure_write_rejects_symlink_at_target` and
  `secure_write_follow_symlinks_writes_through_link` cover both
  paths. Operators should still ensure `<data_dir>` is owned by the
  node user with mode `0o700`.
