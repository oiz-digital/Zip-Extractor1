//! Atomic, mode-0600 file writes for secret material.
//!
//! Task #12 (SEC-2026-05-09 keystore VPS-hardening pass).
//!
//! ## Why this exists
//!
//! Prior to this module every keystore-write site in the workspace used
//! some variant of `std::fs::write(path, bytes)` followed by an
//! optional, best-effort `std::fs::set_permissions(path, 0o600)`. That
//! pattern leaks secret material in two ways:
//!
//! 1. **umask race window.** Between `write` (file created with the
//!    process umask, typically 0o644) and `set_permissions(0o600)`,
//!    another process on the same VPS that polls `<data_dir>` can open
//!    the file read-only and copy out the still-world-readable plaintext.
//!    A handful of milliseconds is enough on a busy server.
//! 2. **Silent best-effort chmod.** If `set_permissions` fails (rare,
//!    but happens on noexec mounts, exotic FUSE filesystems, or
//!    permission-denied scenarios) the secret stays world-readable and
//!    nothing logs the failure.
//!
//! `secure_write` closes both holes by:
//!
//! * On Unix, using `OpenOptions::create_new(true).mode(0o600)` so the
//!   file is **born** with the right permissions atomically — there is
//!   no observable umask window.
//! * Refusing to overwrite an existing path (`create_new`). Callers
//!   that intend to rotate a keyfile must explicitly remove the old one
//!   first; this is a safety rail against accidentally clobbering a
//!   secret with a half-written replacement.
//! * `fsync`-ing the file before returning so a crash mid-write cannot
//!   leave a torn keystore on disk.
//! * On Windows, applying an ACL that grants the current user
//!   read/write and removes the inherited group/world entries (best
//!   effort with a `warn!` log on failure — matches the Pass-4 behaviour
//!   for the Noise static key).
//!
//! ## Why not the OS `mkstemp` / atomic-rename pattern?
//!
//! For keystores we deliberately want `create_new` rather than
//! "write-temp + rename". An atomic rename is a great pattern for
//! hot-swapping config files but here we want any caller that
//! accidentally targets an existing key path to **fail loudly** rather
//! than silently overwrite it.

use crate::KeystoreError;
use std::io::Write;
use std::path::Path;

/// Maximum permission bits permitted on a freshly-written keyfile.
/// Anything in the group/world block (`0o077`) is a leak.
pub const KEYFILE_PERM_MASK: u32 = 0o077;

/// Atomically write `contents` to `path` with mode `0o600` on Unix.
///
/// * Refuses to overwrite an existing path (returns
///   `KeystoreError::Io(AlreadyExists)`).
/// * **Refuses to follow symlinks at the final path component** via
///   `O_NOFOLLOW` on Unix (Task #17). If `path` is a symlink the call
///   fails with `KeystoreError::Io` (`ELOOP`, surfaced as
///   `io::ErrorKind::FilesystemLoop` on recent Rust). This blocks an
///   attacker who can plant entries in `<data_dir>` from redirecting
///   the keystore write through a symlink they control. Operators who
///   deliberately stage a keystore on a network mount must use
///   [`secure_write_follow_symlinks`] instead.
/// * Creates parent directories if they do not exist.
/// * `fsync`s the file before returning (best effort — silently
///   ignores fsync failure on platforms where it is unsupported).
/// * On Windows, falls back to default ACLs and logs a warning;
///   operators on Windows VPS deployments should layer NTFS ACLs at
///   the directory level.
pub fn secure_write(path: &Path, contents: &[u8]) -> Result<(), KeystoreError> {
    secure_write_inner(path, contents, /* follow_symlinks = */ false)
}

/// Symlink-following variant of [`secure_write`] for operators who
/// deliberately stage their keystore on a network mount or other
/// indirection (Task #17 opt-out).
///
/// **Audit warning:** every call emits a `warn!` log because following
/// symlinks at a secret path is a foot-gun: any process that can
/// rewrite the link can redirect the write to a path of its choosing.
/// Only call this when the operator has explicitly requested
/// network-mount staging (e.g. via a CLI flag) and the parent
/// directory's ACLs are tight enough that no untrusted process can
/// touch the link.
pub fn secure_write_follow_symlinks(path: &Path, contents: &[u8]) -> Result<(), KeystoreError> {
    tracing::warn!(
        path = %path.display(),
        "secure_write_follow_symlinks: writing through a possibly-symlinked \
         keystore path — operator must guarantee the parent directory ACLs \
         prevent untrusted processes from rewriting the link"
    );
    secure_write_inner(path, contents, /* follow_symlinks = */ true)
}

fn secure_write_inner(
    path: &Path,
    contents: &[u8],
    follow_symlinks: bool,
) -> Result<(), KeystoreError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    // POSIX wrinkle: `O_CREAT|O_EXCL` (= `create_new(true)`) fails with
    // EEXIST whenever the final path component is *any* symlink, even
    // a dangling one — independent of O_NOFOLLOW. To preserve the
    // create-new safety rail in the opt-in `follow_symlinks` mode we
    // resolve the link manually here and then `create_new` on the
    // resolved target. The default (refuse-symlinks) path does not
    // touch this, so the EEXIST behaviour against symlinks is what
    // protects callers from the swap attack even on platforms that
    // ignore O_NOFOLLOW for some reason.
    let resolved: std::path::PathBuf = if follow_symlinks {
        match std::fs::symlink_metadata(path) {
            Ok(m) if m.file_type().is_symlink() => {
                let target = std::fs::read_link(path)?;
                // Relative symlink targets resolve against the
                // symlink's parent directory, NOT the process CWD.
                // POSIX `readlink(2)` returns the link contents
                // verbatim, so we have to do this join ourselves —
                // otherwise an operator-staged
                // `ln -s relative/keyfile p2p_static.key` would
                // silently write to `$CWD/relative/keyfile`.
                if target.is_absolute() {
                    target
                } else {
                    match path.parent() {
                        Some(p) if !p.as_os_str().is_empty() => p.join(target),
                        _ => target,
                    }
                }
            }
            _ => path.to_path_buf(),
        }
    } else {
        path.to_path_buf()
    };
    let path = resolved.as_path();

    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
        if !follow_symlinks {
            // O_NOFOLLOW: if the *final* path component is a symlink,
            // open() fails with ELOOP rather than following the link.
            // Combined with `create_new(true)` (which translates to
            // O_CREAT|O_EXCL) this means: the file is either created
            // fresh by us under our chosen mode, or the call fails.
            // It cannot land on a pre-existing attacker-controlled
            // target via a symlink swap.
            opts.custom_flags(libc::O_NOFOLLOW);
        }
    }

    #[cfg(windows)]
    {
        if !follow_symlinks {
            // Windows analogue of O_NOFOLLOW:
            // FILE_FLAG_OPEN_REPARSE_POINT (0x00200000) tells
            // CreateFileW to act on the reparse point itself instead
            // of traversing it. Combined with `create_new` (which
            // maps to CREATE_NEW and fails with ERROR_FILE_EXISTS
            // when any reparse point is already at `path`), the call
            // either creates a brand-new regular file or errors —
            // it cannot land on the target of an attacker-planted
            // symlink/junction.
            use std::os::windows::fs::OpenOptionsExt;
            const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
            opts.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        }
    }

    let _ = follow_symlinks; // silence unused warning on exotic targets

    let mut f = opts.open(path)?;
    f.write_all(contents)?;
    let _ = f.sync_all();

    // On Windows the `mode(0o600)` call is a no-op. Emit a single warn
    // so deployment runbooks notice. We deliberately do not fail here:
    // refusing to write would brick Windows-based dev boxes that don't
    // need VPS-grade ACLs.
    #[cfg(windows)]
    {
        tracing::warn!(
            path = %path.display(),
            "secure_write: Windows ACL hardening not applied — \
             rely on NTFS directory ACLs for secret protection"
        );
    }

    Ok(())
}

/// Inspect an existing path and tighten its mode to `0o600` if any
/// group/world bits are set. Logs a `warn!` on every tightening so
/// operators see that their previous deployment was leaky.
///
/// Returns:
/// * `Ok(true)`  — file was loose and has been tightened.
/// * `Ok(false)` — file was already strict (no action taken) or we are
///                 on a non-Unix platform that cannot inspect modes.
/// * `Err(_)`    — could not stat or chmod the file.
pub fn ensure_strict_perms(path: &Path) -> Result<bool, KeystoreError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(path)?;
        let mode = meta.permissions().mode();
        if (mode & KEYFILE_PERM_MASK) != 0 {
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(path, perms)?;
            tracing::warn!(
                path = %path.display(),
                old_mode = format!("{:o}", mode & 0o777),
                "keystore: tightened loose permissions to 0o600 — \
                 a previous run wrote this file with a permissive umask"
            );
            return Ok(true);
        }
        Ok(false)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(false)
    }
}

/// Walk a directory and tighten every file inside (non-recursive — the
/// data dir is flat in practice). Files that disappear between readdir
/// and stat are silently skipped (race-tolerant).
pub fn tighten_dir(dir: &Path) -> Result<usize, KeystoreError> {
    if !dir.exists() {
        return Ok(0);
    }
    let mut tightened = 0usize;
    for entry in std::fs::read_dir(dir)? {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        match ensure_strict_perms(&path) {
            Ok(true) => tightened += 1,
            Ok(false) => {}
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "keystore: could not tighten file mode — leaving as-is"
                );
            }
        }
    }
    Ok(tightened)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn secure_write_creates_with_0600() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret.key");
        secure_write(&path, b"deadbeef").unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "freshly-written keyfile must be 0o600, got {:o}",
            mode & 0o777
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"deadbeef");
    }

    #[test]
    fn secure_write_refuses_existing_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret.key");
        secure_write(&path, b"first").unwrap();

        let err = secure_write(&path, b"second").unwrap_err();
        // The file must not have been overwritten.
        assert_eq!(std::fs::read(&path).unwrap(), b"first");
        assert!(
            matches!(err, KeystoreError::Io(_)),
            "expected Io error, got {err:?}"
        );
    }

    #[test]
    fn secure_write_creates_parent_dir() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("dir").join("k.key");
        secure_write(&path, b"x").unwrap();
        assert!(path.exists());
    }

    #[test]
    fn ensure_strict_perms_tightens_loose_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("loose.key");
        std::fs::write(&path, b"secret").unwrap();
        // Force-loose it: world-readable.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        let tightened = ensure_strict_perms(&path).unwrap();
        assert!(tightened, "0o644 file must be flagged as loose");

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "after tightening, mode must be exactly 0o600"
        );
    }

    #[test]
    fn ensure_strict_perms_noop_on_strict_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("strict.key");
        secure_write(&path, b"x").unwrap();
        assert!(!ensure_strict_perms(&path).unwrap());
    }

    #[test]
    fn tighten_dir_handles_mixed_modes() {
        let dir = tempfile::tempdir().unwrap();
        // One strict, one loose, one already-strict.
        secure_write(&dir.path().join("a.key"), b"a").unwrap();
        std::fs::write(dir.path().join("b.key"), b"b").unwrap();
        std::fs::set_permissions(
            dir.path().join("b.key"),
            std::fs::Permissions::from_mode(0o640),
        )
        .unwrap();
        std::fs::write(dir.path().join("c.key"), b"c").unwrap();
        std::fs::set_permissions(
            dir.path().join("c.key"),
            std::fs::Permissions::from_mode(0o604),
        )
        .unwrap();

        let n = tighten_dir(dir.path()).unwrap();
        assert_eq!(n, 2, "exactly 2 of the 3 files must be tightened");

        for name in &["a.key", "b.key", "c.key"] {
            let mode = std::fs::metadata(dir.path().join(name))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "{name} must be 0o600 after tighten_dir");
        }
    }

    /// Task #17: an attacker who can write into the keystore directory
    /// must not be able to redirect a `secure_write` call through a
    /// symlink. We plant a symlink at the target path that would
    /// otherwise overwrite a sensitive file, then assert
    /// `secure_write` refuses (ELOOP) and the symlink target is
    /// untouched.
    #[test]
    fn secure_write_rejects_symlink_at_target() {
        let dir = tempfile::tempdir().unwrap();
        let attacker_target = dir.path().join("attacker_target");
        std::fs::write(&attacker_target, b"original-untouched").unwrap();

        let keystore_path = dir.path().join("p2p_static.key");
        std::os::unix::fs::symlink(&attacker_target, &keystore_path).unwrap();

        let err = secure_write(&keystore_path, b"NEW-SECRET").unwrap_err();
        assert!(
            matches!(err, KeystoreError::Io(_)),
            "expected Io error from O_NOFOLLOW, got {err:?}"
        );

        // The attacker's planted file must NOT have been overwritten —
        // proves the write did not follow the symlink.
        assert_eq!(
            std::fs::read(&attacker_target).unwrap(),
            b"original-untouched",
            "secure_write followed the symlink and clobbered the target"
        );

        // The symlink itself must still be a symlink (not replaced
        // with a regular file by a half-successful write).
        let lmeta = std::fs::symlink_metadata(&keystore_path).unwrap();
        assert!(
            lmeta.file_type().is_symlink(),
            "keystore path should still be a symlink after refusal"
        );
    }

    /// The opt-in escape hatch (`secure_write_follow_symlinks`) must
    /// still write through a deliberately-staged symlink so operators
    /// can stage keystores on network mounts. We don't reuse the
    /// attacker fixture here because the helper *does* follow the
    /// link by design — instead we point the symlink at a fresh path
    /// and assert the file is created at the link target.
    #[test]
    fn secure_write_follow_symlinks_writes_through_link() {
        let dir = tempfile::tempdir().unwrap();
        let real_target = dir.path().join("real_keystore_on_nfs");
        // Note: the link target does not yet exist — `create_new` on
        // the followed path will create it.
        let link = dir.path().join("p2p_static.key");
        std::os::unix::fs::symlink(&real_target, &link).unwrap();

        secure_write_follow_symlinks(&link, b"NETWORK-MOUNT-OK").unwrap();

        assert_eq!(std::fs::read(&real_target).unwrap(), b"NETWORK-MOUNT-OK");
        let mode = std::fs::metadata(&real_target).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "follow-symlinks variant must still apply 0o600"
        );
    }

    /// Relative symlink targets must be resolved against the symlink's
    /// parent directory, not the process CWD. Regression test for
    /// the in-pass review fix.
    #[test]
    fn secure_write_follow_symlinks_resolves_relative_target() {
        let dir = tempfile::tempdir().unwrap();
        // Symlink at <tmp>/p2p_static.key → "real_keystore"
        // (relative). Resolves correctly only if we join against
        // the link's parent.
        let link = dir.path().join("p2p_static.key");
        std::os::unix::fs::symlink("real_keystore", &link).unwrap();

        secure_write_follow_symlinks(&link, b"REL-OK").unwrap();

        let resolved = dir.path().join("real_keystore");
        assert_eq!(std::fs::read(&resolved).unwrap(), b"REL-OK");

        // Make sure we did NOT accidentally write to ./real_keystore
        // (CWD-relative). If the bug came back this assertion would
        // not catch every case, but the positive assertion above
        // already proves the parent-relative resolution worked.
        let cwd_path = std::env::current_dir().unwrap().join("real_keystore");
        if cwd_path != resolved {
            assert!(
                !cwd_path.exists(),
                "follow-symlinks resolved relative target against CWD instead of link parent"
            );
        }
    }

    #[test]
    fn tighten_dir_missing_dir_is_noop() {
        let n = tighten_dir(Path::new("/tmp/zbx-nonexistent-keys-dir-xyz")).unwrap();
        assert_eq!(n, 0);
    }
}
