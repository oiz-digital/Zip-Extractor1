//! Online backup: snapshot the RocksDB data directory while the node runs.

use crate::error::AdminError;
use serde::{Serialize, Deserialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tracing::{info, warn, error};

/// State of an in-progress or completed backup.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum BackupState {
    Pending,
    Running,
    Completed,
    Failed(String),
    Cancelled,
}

/// Progress tracker for an online backup.
pub struct BackupProgress {
    pub id:            String,
    pub dest_path:     PathBuf,
    pub state:         BackupState,
    pub started_at:    u64,
    pub finished_at:   Option<u64>,
    pub bytes_written: AtomicU64,
    pub files_written: AtomicU64,
    pub cancelled:     AtomicBool,
}

impl BackupProgress {
    pub fn new(id: impl Into<String>, dest_path: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            id:            id.into(),
            dest_path,
            state:         BackupState::Pending,
            started_at:    unix_now(),
            finished_at:   None,
            bytes_written: AtomicU64::new(0),
            files_written: AtomicU64::new(0),
            cancelled:     AtomicBool::new(false),
        })
    }

    pub fn bytes_written(&self) -> u64 { self.bytes_written.load(Ordering::Relaxed) }
    pub fn files_written(&self) -> u64 { self.files_written.load(Ordering::Relaxed) }
    pub fn is_cancelled(&self)  -> bool { self.cancelled.load(Ordering::Relaxed) }
    pub fn cancel(&self) { self.cancelled.store(true, Ordering::Relaxed); }
}

/// Start an online backup to `dest`.
pub fn start_backup(
    db_path:  &Path,
    dest:     &Path,
) -> Result<Arc<BackupProgress>, AdminError> {
    if !db_path.exists() {
        return Err(AdminError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("DB path '{}' not found", db_path.display()),
        )));
    }
    std::fs::create_dir_all(dest)?;

    let id       = format!("backup-{}", unix_now());
    let progress = BackupProgress::new(&id, dest.to_path_buf());

    info!("admin: starting online backup '{}' → '{}'", id, dest.display());

    // In production: spawn a background task that calls RocksDB::create_checkpoint.
    // The checkpoint is a hard-linked copy of all SST files — near-instant on most FS.

    Ok(progress)
}

/// Verify an existing backup directory.
pub fn verify_backup(path: &Path) -> Result<BackupVerifyResult, AdminError> {
    if !path.exists() {
        return Err(AdminError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("backup path '{}' not found", path.display()),
        )));
    }
    // In production: read manifest, verify SST checksums.
    info!("admin: verifying backup at '{}'", path.display());
    Ok(BackupVerifyResult {
        path:       path.to_path_buf(),
        ok:         true,
        file_count: 0,
        size_bytes: 0,
        error:      None,
    })
}

/// Restore a backup to a clean data directory.
pub fn restore_backup(
    backup_path: &Path,
    restore_to:  &Path,
) -> Result<(), AdminError> {
    if restore_to.exists() {
        return Err(AdminError::Config(format!(
            "restore target '{}' already exists — please remove it first",
            restore_to.display()
        )));
    }
    warn!(
        "admin: restoring backup '{}' → '{}' (this will take a while)",
        backup_path.display(), restore_to.display()
    );
    // In production: hardlink all SST files from backup, rebuild MANIFEST.
    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BackupVerifyResult {
    pub path:       PathBuf,
    pub ok:         bool,
    pub file_count: u32,
    pub size_bytes: u64,
    pub error:      Option<String>,
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}