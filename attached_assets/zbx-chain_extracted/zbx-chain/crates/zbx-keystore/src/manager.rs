//! Keystore manager — manages a directory of keystore files.

use crate::{KeyFile, KeystoreError, KeystoreWallet};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Manages a collection of keystores on disk (one per account).
pub struct KeystoreManager {
    dir:       PathBuf,
    keystores: HashMap<[u8; 20], KeyFile>,
}

impl KeystoreManager {
    pub fn new<P: AsRef<Path>>(dir: P) -> Result<Self, KeystoreError> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir)?;
        let mut mgr = Self { dir, keystores: HashMap::new() };
        mgr.load_all()?;
        Ok(mgr)
    }

    /// Load all keystore files from the directory.
    fn load_all(&mut self) -> Result<(), KeystoreError> {
        for entry in std::fs::read_dir(&self.dir)? {
            let entry = entry?;
            if entry.path().extension().map(|e| e == "json").unwrap_or(false) {
                let data = std::fs::read(entry.path())?;
                if let Ok(kf) = KeyFile::from_json(&data) {
                    if let Ok(addr) = kf.address_bytes() {
                        self.keystores.insert(addr, kf);
                    }
                }
            }
        }
        Ok(())
    }

    /// Get all managed addresses.
    pub fn accounts(&self) -> Vec<[u8; 20]> {
        self.keystores.keys().cloned().collect()
    }

    /// Unlock a keystore (decrypt private key) and return a wallet.
    pub fn unlock(&self, address: &[u8; 20], password: &str) -> Result<KeystoreWallet, KeystoreError> {
        let kf = self.keystores.get(address)
            .ok_or_else(|| KeystoreError::NotFound(hex::encode(address)))?;
        KeystoreWallet::from_keyfile(kf, password)
    }

    pub fn has_account(&self, address: &[u8; 20]) -> bool {
        self.keystores.contains_key(address)
    }

    pub fn count(&self) -> usize { self.keystores.len() }
}