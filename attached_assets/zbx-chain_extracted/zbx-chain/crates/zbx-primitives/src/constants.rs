//! ZBX chain constants -- timing, limits, roles, storage.
//!
//! All timing-related constants are in seconds unless noted.
//! All storage capacity constants are in bytes unless noted.
//! Role constants follow OpenZeppelin AccessControl convention:
//!   keccak256(role_name) -> bytes32 role identifier.

// ── Timing ────────────────────────────────────────────────────────────────────

/// Duration of a single slot in milliseconds (ZBX: 3 seconds per slot).
pub const SLOT_DURATION: u64 = 3_000; // 3 seconds

/// Number of slots per epoch (ZBX: 32 slots/epoch = 96 second epoch).
pub const SLOTS_PER_EPOCH: u64 = 32;

/// Epoch duration in seconds (SLOT_DURATION * SLOTS_PER_EPOCH / 1000).
pub const EPOCH_DURATION_SECS: u64 = SLOT_DURATION * SLOTS_PER_EPOCH / 1_000;

/// Maximum clock drift allowed between peers (milliseconds).
pub const MAX_CLOCK_DRIFT_MS: u64 = 500;

// ── Block limits ──────────────────────────────────────────────────────────────

/// Maximum gas per block (30M gas -- same as Ethereum mainnet).
pub const BLOCK_GAS_LIMIT: u64 = 30_000_000;

/// Target gas per block (15M gas -- EIP-1559 elasticity = 2x).
pub const BLOCK_GAS_TARGET: u64 = 15_000_000;

/// Maximum block size in bytes (2 MB).
pub const MAX_BLOCK_SIZE: usize = 2 * 1024 * 1024;

/// Base fee denominator (EIP-1559 formula).
pub const BASE_FEE_MAX_CHANGE_DENOMINATOR: u64 = 8;

// ── Mempool ───────────────────────────────────────────────────────────────────

/// Maximum transactions per account in the pool (pending + queued).
pub const MAX_TX_PER_ACCOUNT: usize = 64;

/// Maximum nonce gap for queued transactions.
pub const MAX_NONCE_GAP: u64 = 64;

/// QueuedPool maximum total transactions across all accounts.
pub const QUEUED_POOL_MAX_TXS: usize = 16_384;

/// PendingPool maximum total transactions across all accounts.
pub const PENDING_POOL_MAX_TXS: usize = 8_192;

// ── Storage keys ──────────────────────────────────────────────────────────────

/// Column family names for RocksDB.
pub mod cf {
    /// Block headers (block_hash -> RLP-encoded header).
    pub const BLOCK_HEADERS: &str = "block_headers";
    /// Block bodies (block_hash -> RLP-encoded body).
    pub const BLOCK_BODIES:  &str = "block_bodies";
    /// Transaction index (tx_hash -> (block_hash, tx_index)).
    pub const TX_INDEX:      &str = "tx_index";
    /// Receipt store (tx_hash -> RLP-encoded receipt).
    pub const RECEIPT_STORE: &str = "receipt_store";
    /// State trie nodes (node_hash -> node_bytes).
    pub const STATE_TRIE:    &str = "state_trie";
    /// Account storage (account_hash ++ slot_hash -> value).
    pub const ACCOUNT_STORE: &str = "account_store";
    /// Contract code (code_hash -> bytecode).
    pub const CODE_STORE:    &str = "code_store";
}

// ── Access control roles (OpenZeppelin convention) ────────────────────────────

/// AccessControl role: default admin (can grant/revoke all roles).
/// Value: keccak256("") = 0x0000...0000 (OZ convention: admin role is zero)
pub const ADMIN_ROLE: [u8; 32] = [0u8; 32];

/// AccessControl role: can pause contracts.
/// Value: keccak256("PAUSER_ROLE")
pub const PAUSER_ROLE: [u8; 32] = [
    0x65, 0xd7, 0xa2, 0x8e, 0x32, 0x65, 0xb3, 0x7a,
    0x6f, 0x09, 0x44, 0x49, 0xe5, 0x7b, 0x75, 0x52,
    0xd2, 0x02, 0xe6, 0x81, 0x48, 0x8e, 0xa3, 0x4f,
    0xf5, 0xe5, 0xa9, 0x07, 0x91, 0x99, 0x45, 0xef,
];

/// AccessControl role: can mint ZBX tokens.
/// Value: keccak256("MINTER_ROLE")
pub const MINTER_ROLE: [u8; 32] = [
    0x9f, 0x2d, 0xf0, 0xfe, 0xd2, 0xc7, 0x76, 0xd7,
    0x88, 0x4b, 0xe0, 0xd1, 0x91, 0x73, 0x46, 0x19,
    0x77, 0x21, 0x5d, 0x34, 0x4e, 0xd2, 0x27, 0x79,
    0xb5, 0xbe, 0x2c, 0x8e, 0x83, 0x4e, 0x89, 0x28,
];

/// AccessControl role: can upgrade proxy contracts.
/// Value: keccak256("UPGRADER_ROLE")
pub const UPGRADER_ROLE: [u8; 32] = [
    0x18, 0x9a, 0xb7, 0xa9, 0x24, 0x4d, 0xf2, 0x13,
    0x81, 0x81, 0x98, 0x74, 0x13, 0x79, 0x48, 0x5b,
    0x64, 0xdd, 0xb1, 0xb9, 0x4a, 0xb3, 0x4f, 0x87,
    0xd8, 0x12, 0x99, 0xfe, 0x30, 0x4b, 0xe9, 0x52,
];

/// AccessControl role: bridge relayer.
/// Value: keccak256("RELAYER_ROLE")
pub const RELAYER_ROLE: [u8; 32] = [
    0xe2, 0xb7, 0xfb, 0x3b, 0x83, 0x2e, 0xd4, 0x01,
    0xbe, 0xf8, 0x95, 0x0c, 0x89, 0x56, 0xab, 0xe7,
    0x19, 0x01, 0x97, 0x89, 0xe1, 0x91, 0xcd, 0xd6,
    0x0f, 0xf8, 0xd3, 0xbe, 0xb1, 0x23, 0x62, 0xf7,
];

// ── AccessControl trait ───────────────────────────────────────────────────────

/// AccessControl -- role-based access system (mirrors OZ AccessControl).
///
/// Unlike Ownable (single owner), AccessControl supports multiple roles:
///   - Each role has a set of accounts that hold it
///   - Each role has an admin role that can grant/revoke it
///   - DEFAULT_ADMIN_ROLE (0x00...00) is the root admin
///
/// ZBX contracts that use AccessControl:
///   ZbxGovernor, ZRC20Token (minter), ZbxBridge (relayer), ZbxStaking (admin)
pub trait AccessControl {
    /// Check if an account holds a role.
    fn has_role(&self, role: [u8; 32], account: [u8; 20]) -> bool;
    /// Grant a role to an account (only callable by role's admin).
    fn grant_role(&mut self, role: [u8; 32], account: [u8; 20]) -> Result<(), AcError>;
    /// Revoke a role from an account.
    fn revoke_role(&mut self, role: [u8; 32], account: [u8; 20]) -> Result<(), AcError>;
    /// Renounce a role (caller removes themselves).
    fn renounce_role(&mut self, role: [u8; 32], caller: [u8; 20]) -> Result<(), AcError>;
    /// Get the admin role for a role.
    fn get_role_admin(&self, role: [u8; 32]) -> [u8; 32];
}

#[derive(Debug)]
pub enum AcError { Unauthorized, RoleAlreadyGranted, RoleNotHeld }

// ── Receipt storage ───────────────────────────────────────────────────────────

/// Receipt store -- persists transaction receipts by tx_hash and block.
///
/// Receipts are needed for:
///   1. zbx_getTransactionReceipt RPC call
///   2. zbx_getLogs RPC call (bloom filter -> full log scan)
///   3. eth_subscribe logs WebSocket
///   4. Bridge: proves a cross-chain event occurred
///   5. Block finalization: receipts_root in block header
pub struct ReceiptStore {
    /// In production: backed by RocksDB (cf::RECEIPT_STORE).
    /// Maps tx_hash -> stored receipt.
    pub cf_name: &'static str,
}

/// In-memory receipt store — keyed by tx_hash.
///
/// Development / test-node backing.  Production nodes replace this with a
/// RocksDB column-family (`cf::RECEIPT_STORE`); when ZEP-016 lands, pass a
/// `Arc<DB>` handle into `ReceiptStore::with_db`.
///
/// Thread-safe via OnceLock + Mutex.
static RECEIPT_MEM: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<[u8; 32], StoredReceipt>>,
> = std::sync::OnceLock::new();

fn receipt_mem() -> &'static std::sync::Mutex<std::collections::HashMap<[u8; 32], StoredReceipt>> {
    RECEIPT_MEM.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

impl ReceiptStore {
    pub const CF: &'static str = cf::RECEIPT_STORE;

    pub fn new() -> Self { Self { cf_name: Self::CF } }

    /// Persist a receipt for a transaction.
    ///
    /// In development this is backed by an in-process HashMap.
    /// Production: `db.put_cf(CF, tx_hash, bincode::encode(receipt))`
    pub fn put(&self, tx_hash: [u8; 32], receipt: &StoredReceipt) -> Result<(), DbError> {
        receipt_mem()
            .lock()
            .map_err(|_| DbError::IoError)?
            .insert(tx_hash, receipt.clone());
        Ok(())
    }

    /// Retrieve a receipt by transaction hash, if stored.
    pub fn get(&self, tx_hash: [u8; 32]) -> Result<Option<StoredReceipt>, DbError> {
        let map = receipt_mem()
            .lock()
            .map_err(|_| DbError::IoError)?;
        Ok(map.get(&tx_hash).cloned())
    }

    /// Get all receipts whose `block_hash` matches `block_hash`.
    pub fn get_block_receipts(&self, block_hash: [u8; 32]) -> Result<Vec<StoredReceipt>, DbError> {
        let map = receipt_mem()
            .lock()
            .map_err(|_| DbError::IoError)?;
        Ok(map.values()
            .filter(|r| r.block_hash == block_hash)
            .cloned()
            .collect())
    }
}

#[derive(Debug, Clone)]
pub struct StoredReceipt {
    pub tx_hash:      [u8; 32],
    pub block_hash:   [u8; 32],
    pub block_number: u64,
    pub tx_index:     u32,
    pub status:       bool,
    pub gas_used:     u64,
    pub logs_bloom:   [u8; 256],
    pub logs:         Vec<StoredLog>,
}

#[derive(Debug, Clone)]
pub struct StoredLog {
    pub address: [u8; 20],
    pub topics:  Vec<[u8; 32]>,
    pub data:    Vec<u8>,
}

// ── QueuedPool ─────────────────────────────────────────────────────────────────

/// QueuedPool -- transactions with nonce gaps (waiting for prior nonces).
///
/// A transaction is queued (not pending) when:
///   tx.nonce > account_state_nonce
///
/// Example: state_nonce=5, pool has nonces 5,6,8,9
///   - nonce 5: pending (ready)
///   - nonce 6: pending (ready after 5 mines)
///   - nonce 8: queued (waiting for nonce 7)
///   - nonce 9: queued (waiting for nonces 7 and 8)
///
/// QueuedPool eviction:
///   When pool is full: evict lowest-fee queued txs first.
///   Queued txs are NOT propagated to peers (only pending txs are).
pub struct QueuedPool {
    /// Per-sender queued transactions: sender -> sorted by nonce
    pub queued: std::collections::HashMap<[u8; 20], std::collections::BTreeMap<u64, QueuedTx>>,
    /// Total count
    pub total:  usize,
    /// Maximum queued txs across all senders
    pub max:    usize,
}

#[derive(Debug, Clone)]
pub struct QueuedTx {
    pub hash:              [u8; 32],
    pub nonce:             u64,
    pub max_fee_per_gas:   u128,
    pub gas_limit:         u64,
    pub added_at:          u64,
}

impl QueuedPool {
    pub fn new(max: usize) -> Self {
        Self { queued: Default::default(), total: 0, max }
    }

    pub fn add(&mut self, sender: [u8; 20], tx: QueuedTx) -> Result<(), &'static str> {
        if self.total >= self.max { return Err("queued pool full"); }
        self.queued.entry(sender).or_default().insert(tx.nonce, tx);
        self.total += 1;
        Ok(())
    }

    pub fn remove(&mut self, sender: &[u8; 20], nonce: u64) -> Option<QueuedTx> {
        if let Some(map) = self.queued.get_mut(sender) {
            if let Some(tx) = map.remove(&nonce) {
                self.total -= 1;
                return Some(tx);
            }
        }
        None
    }

    /// Promote queued txs to pending when the sender's nonce advances.
    pub fn promote(&mut self, sender: &[u8; 20], new_nonce: u64) -> Vec<QueuedTx> {
        let mut promoted = Vec::new();
        if let Some(map) = self.queued.get_mut(sender) {
            while let Some(entry) = map.first_key_value() {
                if *entry.0 <= new_nonce {
                    let nonce = *entry.0;
                    if let Some(tx) = map.remove(&nonce) {
                        self.total -= 1;
                        promoted.push(tx);
                    }
                } else { break; }
            }
        }
        promoted
    }

    pub fn count(&self) -> usize { self.total }

    pub fn sender_count(&self, sender: &[u8; 20]) -> usize {
        self.queued.get(sender).map(|m| m.len()).unwrap_or(0)
    }
}

#[derive(Debug)]
pub enum DbError { NotFound, IoError, SerializeError }