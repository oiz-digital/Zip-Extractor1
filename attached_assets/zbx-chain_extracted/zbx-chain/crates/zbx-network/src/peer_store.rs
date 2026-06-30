//! Persistent peer + ban store.
//!
//! SEC-2026-05-09 Pass-10. Prior implementation kept the known-peer table
//! and the ban-set entirely in memory: every node restart wiped the
//! reputation history we'd built up, so a freshly-restarted validator
//! would happily redial peers it had banned five seconds earlier for
//! BadHandshake / InvalidQC. This module persists both sets atomically
//! to disk so the operator's accumulated trust survives restart.
//!
//! ## On-disk layout
//!
//! ```text
//! <data_dir>/p2p/
//!   peers.json     — last-seen address book (best-block, latency, etc.)
//!   banlist.json   — banned peer IDs + IP addrs with TTL
//! ```
//!
//! Both files are written via `tmp + rename` so a crash mid-write never
//! produces a half-written file. File mode is `0600` on Unix to keep peer
//! IPs out of any process running as a different uid.
//!
//! ## TTL
//!
//! Bans have a configurable TTL (default 24h). Expired entries are dropped
//! at load time. A permanent ban is encoded as `ttl_secs = 0`.

use crate::peer::{PeerId, PeerInfo};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, error, info, warn};

/// Default ban duration (24h). 0 = permanent.
pub const DEFAULT_BAN_TTL_SECS: u64 = 24 * 3600;

/// Cap on banlist size — older entries are evicted FIFO above this.
pub const MAX_BANLIST_ENTRIES: usize = 10_000;

/// Cap on peer-store size.
pub const MAX_PEER_STORE_ENTRIES: usize = 5_000;

/// One ban entry — peer id + originating IP + TTL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistentBan {
    pub peer_id:    Option<PeerId>,
    pub ip:         Option<IpAddr>,
    pub reason:     String,
    pub banned_at:  u64,   // unix seconds
    pub ttl_secs:   u64,   // 0 = permanent
}

impl PersistentBan {
    pub fn is_expired(&self, now_secs: u64) -> bool {
        self.ttl_secs != 0 && now_secs.saturating_sub(self.banned_at) >= self.ttl_secs
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PeerFile {
    peers: Vec<PeerInfo>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct BanFile {
    bans: Vec<PersistentBan>,
}

/// Persistent peer + ban store.
pub struct PeerStore {
    base_dir:    PathBuf,
    peers_path:  PathBuf,
    bans_path:   PathBuf,
    /// Cached known-peer entries.
    pub peers:   HashMap<PeerId, PeerInfo>,
    /// Cached banlist (in-memory mirror of disk).
    pub bans:    Vec<PersistentBan>,
}

impl PeerStore {
    /// Open (creating if necessary) the peer store under `<data_dir>/p2p`.
    pub fn open(base_dir: impl Into<PathBuf>) -> std::io::Result<Self> {
        let base: PathBuf = base_dir.into();
        let p2p_dir = base.join("p2p");
        fs::create_dir_all(&p2p_dir)?;
        let peers_path = p2p_dir.join("peers.json");
        let bans_path  = p2p_dir.join("banlist.json");

        // SEC-2026-05-09 Pass-10 (architect review follow-up) — fail loud
        // on banlist read/parse errors. Silently degrading to an empty
        // banlist is an access-control weakening (reboot = forgiveness).
        // ENOENT (first boot) is the only acceptable "treat as empty"
        // case; everything else (perm denied, corrupt JSON, truncated)
        // must abort node startup so an operator looks at the file.
        let peers = match load_peers(&peers_path) {
            Ok(p) => p,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Default::default(),
            Err(e) => {
                error!(path = %peers_path.display(), error = %e,
                    "PeerStore: refusing to start with corrupt peers.json");
                return Err(e);
            }
        };
        let mut bans = match load_bans(&bans_path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Default::default(),
            Err(e) => {
                error!(path = %bans_path.display(), error = %e,
                    "PeerStore: refusing to start with corrupt banlist.json \
                     (silent fallback to empty bans is an access-control weakening)");
                return Err(e);
            }
        };
        let now = unix_now();
        let before = bans.len();
        bans.retain(|b| !b.is_expired(now));
        let dropped = before - bans.len();
        if dropped > 0 {
            info!(dropped, "PeerStore: pruned {} expired bans on load", dropped);
        }
        info!(peers = peers.len(), bans = bans.len(),
            "PeerStore: opened {}", p2p_dir.display());
        Ok(Self {
            base_dir: p2p_dir,
            peers_path,
            bans_path,
            peers,
            bans,
        })
    }

    /// Open in a private temp directory — for tests only. Uses a process-
    /// wide atomic counter so parallel tests inside the same `cargo test`
    /// invocation cannot collide (same pid, same second).
    #[cfg(test)]
    pub fn open_tempdir() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "zbx_peer_store_test_{}_{}_{}",
            std::process::id(),
            unix_now(),
            n,
        ));
        // Defensive: if the path somehow already exists (re-running tests
        // very quickly without cleanup), wipe it first so the test starts
        // from a known-empty state.
        let _ = fs::remove_dir_all(&dir);
        Self::open(dir).expect("temp peer store")
    }

    /// Add or refresh a peer in the address book.
    pub fn record_peer(&mut self, info: PeerInfo) -> std::io::Result<()> {
        if self.peers.len() >= MAX_PEER_STORE_ENTRIES && !self.peers.contains_key(&info.id) {
            // Evict oldest by `last_seen` to make room.
            if let Some((oldest, _)) = self.peers.iter()
                .min_by_key(|(_, v)| v.last_seen)
                .map(|(k, _)| (k.clone(), ()))
            {
                self.peers.remove(&oldest);
            }
        }
        self.peers.insert(info.id.clone(), info);
        self.save_peers()
    }

    /// Add a ban entry. `ttl_secs == 0` ⇒ permanent.
    pub fn ban(
        &mut self,
        peer_id: Option<PeerId>,
        ip: Option<IpAddr>,
        reason: impl Into<String>,
        ttl_secs: u64,
    ) -> std::io::Result<()> {
        let entry = PersistentBan {
            peer_id: peer_id.clone(),
            ip,
            reason: reason.into(),
            banned_at: unix_now(),
            ttl_secs,
        };
        warn!(?peer_id, ?ip, reason = %entry.reason, ttl_secs,
            "PeerStore: persisting ban");
        self.bans.push(entry);
        // Cap banlist size — evict oldest first.
        if self.bans.len() > MAX_BANLIST_ENTRIES {
            self.bans.sort_by_key(|b| b.banned_at);
            let drop = self.bans.len() - MAX_BANLIST_ENTRIES;
            self.bans.drain(..drop);
        }
        self.save_bans()
    }

    /// Drop a peer from the address book (clean disconnect, etc.).
    pub fn forget_peer(&mut self, id: &PeerId) -> std::io::Result<()> {
        if self.peers.remove(id).is_some() {
            self.save_peers()
        } else {
            Ok(())
        }
    }

    /// Is this peer / IP currently banned?
    pub fn is_banned_id(&self, id: &PeerId) -> bool {
        let now = unix_now();
        self.bans.iter().any(|b| !b.is_expired(now)
            && b.peer_id.as_ref() == Some(id))
    }

    pub fn is_banned_ip(&self, ip: &IpAddr) -> bool {
        let now = unix_now();
        self.bans.iter().any(|b| !b.is_expired(now)
            && b.ip.as_ref() == Some(ip))
    }

    /// Sweep expired bans, persisting if anything was removed.
    pub fn prune_expired(&mut self) -> std::io::Result<usize> {
        let now = unix_now();
        let before = self.bans.len();
        self.bans.retain(|b| !b.is_expired(now));
        let removed = before - self.bans.len();
        if removed > 0 {
            self.save_bans()?;
            debug!(removed, "PeerStore: pruned expired bans");
        }
        Ok(removed)
    }

    fn save_peers(&self) -> std::io::Result<()> {
        let file = PeerFile { peers: self.peers.values().cloned().collect() };
        let bytes = serde_json::to_vec_pretty(&file)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        atomic_write(&self.peers_path, &bytes)
    }

    fn save_bans(&self) -> std::io::Result<()> {
        let file = BanFile { bans: self.bans.clone() };
        let bytes = serde_json::to_vec_pretty(&file)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        atomic_write(&self.bans_path, &bytes)
    }

    pub fn dir(&self) -> &Path { &self.base_dir }
    pub fn ban_count(&self) -> usize { self.bans.len() }
    pub fn peer_count(&self) -> usize { self.peers.len() }
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn load_peers(path: &Path) -> std::io::Result<HashMap<PeerId, PeerInfo>> {
    if !path.exists() { return Ok(HashMap::new()); }
    let bytes = fs::read(path)?;
    let file: PeerFile = serde_json::from_slice(&bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    Ok(file.peers.into_iter().map(|p| (p.id.clone(), p)).collect())
}

fn load_bans(path: &Path) -> std::io::Result<Vec<PersistentBan>> {
    if !path.exists() { return Ok(Vec::new()); }
    let bytes = fs::read(path)?;
    let file: BanFile = serde_json::from_slice(&bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    Ok(file.bans)
}

/// Write `data` to `path` atomically (tmp + rename + fsync).
///
/// On Unix the tmp file is created with 0600 mode so peer/IP data can't
/// be read by other uids on a shared host. Fsync the file *and* the
/// parent dir so a crash never leaves a half-written manifest.
fn atomic_write(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| std::io::Error::new(
        std::io::ErrorKind::InvalidInput, "peer store path has no parent"))?;
    let tmp = parent.join(format!(
        ".{}.tmp.{}",
        path.file_name().and_then(|f| f.to_str()).unwrap_or("peer_store"),
        std::process::id(),
    ));

    {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
    }

    fs::rename(&tmp, path)?;
    // Fsync the parent dir so the rename itself is durable.
    if let Ok(dir) = fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn pid(b: u8) -> PeerId { PeerId([b; 32]) }

    #[test]
    fn roundtrip_bans_persist_across_reopen() {
        let store = PeerStore::open_tempdir();
        let dir = store.base_dir.parent().unwrap().to_path_buf();
        drop(store);

        // Open, add ban, close.
        {
            let mut s = PeerStore::open(&dir).unwrap();
            s.ban(Some(pid(7)), Some(IpAddr::V4(Ipv4Addr::new(10,0,0,1))),
                "BadHandshake", 3600).unwrap();
            assert_eq!(s.ban_count(), 1);
        }

        // Reopen — ban should still be there.
        {
            let s = PeerStore::open(&dir).unwrap();
            assert_eq!(s.ban_count(), 1);
            assert!(s.is_banned_id(&pid(7)));
            assert!(s.is_banned_ip(&IpAddr::V4(Ipv4Addr::new(10,0,0,1))));
            assert!(!s.is_banned_ip(&IpAddr::V4(Ipv4Addr::new(10,0,0,2))));
        }
    }

    #[test]
    fn expired_bans_pruned_on_load() {
        let store = PeerStore::open_tempdir();
        let dir = store.base_dir.parent().unwrap().to_path_buf();
        drop(store);

        // Manually inject a ban that's already past TTL.
        let p2p_dir = dir.join("p2p");
        fs::create_dir_all(&p2p_dir).unwrap();
        let bans = BanFile { bans: vec![PersistentBan {
            peer_id: Some(pid(9)),
            ip: None,
            reason: "test-expired".into(),
            banned_at: unix_now() - 7200,
            ttl_secs: 60,   // expired ~119 minutes ago
        }] };
        fs::write(p2p_dir.join("banlist.json"),
            serde_json::to_vec(&bans).unwrap()).unwrap();

        let s = PeerStore::open(&dir).unwrap();
        assert_eq!(s.ban_count(), 0, "expired ban must be dropped on load");
    }

    #[test]
    fn permanent_ban_never_expires() {
        let mut s = PeerStore::open_tempdir();
        s.ban(Some(pid(1)), None, "perma", 0).unwrap();
        assert!(s.bans[0].ttl_secs == 0);
        // Even at year 3000 it should still hold.
        let entry = s.bans[0].clone();
        assert!(!entry.is_expired(u64::MAX / 2));
    }

    /// Verifies the eviction logic without writing 10k+ files to disk
    /// (which is both slow and can race with the test runner cleaning
    /// up tempdirs). We exercise the in-memory cap path directly.
    #[test]
    fn banlist_capped() {
        let mut s = PeerStore::open_tempdir();
        // Stuff the in-memory vector past the cap, then trigger one
        // save which exercises the eviction path.
        for i in 0..MAX_BANLIST_ENTRIES + 5 {
            let mut id = [0u8; 32];
            id[..8].copy_from_slice(&(i as u64).to_be_bytes());
            s.bans.push(PersistentBan {
                peer_id: Some(PeerId(id)),
                ip: None,
                reason: "spam".into(),
                banned_at: unix_now() - (i as u64),
                ttl_secs: 3600,
            });
        }
        // One real ban() call triggers the eviction + save path.
        s.ban(Some(pid(255)), None, "trigger", 3600).unwrap();
        assert!(s.ban_count() <= MAX_BANLIST_ENTRIES,
            "banlist overflow: {}", s.ban_count());
    }
}
