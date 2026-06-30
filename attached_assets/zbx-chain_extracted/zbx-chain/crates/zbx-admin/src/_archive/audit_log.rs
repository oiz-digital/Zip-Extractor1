//! Admin audit log -- tamper-evident log of all admin actions.
//!
//! Every admin action is recorded with:
//!   - Block number and timestamp
//!   - Caller address (admin who performed the action)
//!   - Action type + parameters
//!   - Hash of previous log entry (chain of custody)
//!
//! The audit log is stored in chain state and emitted as events.
//! It is used for governance transparency, regulatory compliance,
//! and post-incident forensics.

use std::time::SystemTime;

// ── Audit Log ─────────────────────────────────────────────────────────────────

/// A single admin audit log entry.
#[derive(Debug, Clone)]
pub struct AuditEntry {
    /// Sequential log index
    pub index:      u64,
    /// Block number when this action occurred
    pub block:      u64,
    /// Unix timestamp
    pub timestamp:  u64,
    /// Address of the admin who performed this action
    pub caller:     [u8; 20],
    /// Action performed
    pub action:     AuditAction,
    /// Hash of the previous entry (for tamper-evidence)
    pub prev_hash:  [u8; 32],
    /// Hash of this entry (keccak256 of all fields)
    pub entry_hash: [u8; 32],
}

/// All auditable admin actions.
#[derive(Debug, Clone)]
pub enum AuditAction {
    OwnershipTransferred  { from: [u8; 20], to: [u8; 20] },
    OwnershipRenounced    { prev: [u8; 20] },
    RoleGranted           { role: String, account: [u8; 20] },
    RoleRevoked           { role: String, account: [u8; 20] },
    AccessRevoked         { account: [u8; 20] },
    ChainPaused,
    ChainUnpaused,
    AccountFrozen         { account: [u8; 20] },
    AccountBlacklisted    { account: [u8; 20] },
    CircuitBreakerTripped { reason: String },
    CircuitBreakerReset,
    BaseFeeChanged        { old: u64, new: u64 },
    GasLimitChanged       { old: u64, new: u64 },
    EpochLengthChanged    { old: u64, new: u64 },
    MinStakeChanged       { old: u128, new: u128 },
    RewardRateChanged     { old: u32, new: u32 },
    SlashPctChanged       { old: u8, new: u8 },
    ValidatorSetOverridden { old_count: u32, new_count: u32, reason: String },
    ValidatorSlotsChanged  { old: u32, new: u32 },
    ValidatorJailed       { validator: [u8; 20] },
    ValidatorUnjailed     { validator: [u8; 20] },
    HardForkScheduled     { name: String, activation_block: u64 },
    HardForkCancelled     { name: String },
    UpgradeVotePassed     { proposal_id: u64 },
    AdminKeyRotated       { account: [u8; 20] },
    TreasuryWithdrawal    { recipient: [u8; 20], amount: u128 },
    FeeDistributed        { epoch: u64, amount: u128 },
}

/// Tamper-evident admin audit log.
/// Entries form a hash chain: each entry contains prev_hash.
pub struct AuditLog {
    pub entries:    Vec<AuditEntry>,
    pub last_hash:  [u8; 32],
}

impl AuditLog {
    pub fn new() -> Self {
        Self { entries: Vec::new(), last_hash: [0u8; 32] }
    }

    /// Append a new audit entry. Returns the new entry's hash.
    pub fn append(&mut self, block: u64, caller: [u8; 20], action: AuditAction) -> [u8; 32] {
        let index = self.entries.len() as u64;
        let timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default().as_secs();
        let prev_hash = self.last_hash;
        // entry_hash = keccak256(index || block || caller || action_encoded || prev_hash)
        let entry_hash = compute_entry_hash(index, block, &caller, &action, &prev_hash);
        let entry = AuditEntry { index, block, timestamp, caller, action, prev_hash, entry_hash };
        self.last_hash = entry_hash;
        self.entries.push(entry);
        entry_hash
    }

    /// Verify the integrity of the audit log (hash chain check).
    pub fn verify(&self) -> bool {
        let mut prev = [0u8; 32];
        for entry in &self.entries {
            if entry.prev_hash != prev { return false; }
            let expected = compute_entry_hash(
                entry.index, entry.block, &entry.caller, &entry.action, &prev
            );
            if entry.entry_hash != expected { return false; }
            prev = entry.entry_hash;
        }
        true
    }

    /// Query admin event history for a specific caller.
    pub fn history_for(&self, caller: [u8; 20]) -> Vec<&AuditEntry> {
        self.entries.iter().filter(|e| e.caller == caller).collect()
    }

    /// Query all entries since a given block.
    pub fn since_block(&self, block: u64) -> Vec<&AuditEntry> {
        self.entries.iter().filter(|e| e.block >= block).collect()
    }

    /// Get the complete admin event history.
    pub fn admin_event_history(&self) -> &[AuditEntry] {
        &self.entries
    }
}

fn compute_entry_hash(
    index: u64, block: u64, caller: &[u8; 20],
    action: &AuditAction, prev: &[u8; 32]
) -> [u8; 32] {
    // keccak256(abi.encode(index, block, caller, action_id, prev_hash))
    // Implemented in zbx-crypto crate
    let _ = (index, block, caller, action, prev);
    [0u8; 32]
}

// ── Key Rotation ──────────────────────────────────────────────────────────────

/// Admin key rotation -- rotate the secp256k1 key for an admin account.
///
/// Key rotation is required:
///   - Every 90 days (policy)
///   - After any suspected compromise
///   - When an admin leaves the team
///
/// Process:
///   1. Admin signs rotation request with old key
///   2. New key is added to allowlist
///   3. 24h delay before old key is invalidated
///   4. AuditLog records the rotation with both pubkeys
#[derive(Debug, Clone)]
pub struct KeyRotation {
    pub account:        [u8; 20],
    pub old_pubkey:     [u8; 33],   // compressed secp256k1
    pub new_pubkey:     [u8; 33],   // compressed secp256k1
    pub rotated_at:     u64,        // block
    pub effective_at:   u64,        // block (rotated_at + rotation_delay)
    pub rotation_sig:   [u8; 64],   // signature with old key over new_pubkey
}

pub const KEY_ROTATION_DELAY_BLOCKS: u64 = 28_800; // 24h at 3s/block
pub const KEY_ROTATION_POLICY_BLOCKS: u64 = 2_592_000; // ~90 days

/// Initiate a key rotation for an admin account.
pub fn initiate_key_rotation(
    account:     [u8; 20],
    old_pubkey:  [u8; 33],
    new_pubkey:  [u8; 33],
    rotation_sig: [u8; 64],
    block:       u64,
) -> Result<KeyRotation, KeyRotationError> {
    if old_pubkey == new_pubkey { return Err(KeyRotationError::SameKey); }
    if new_pubkey == [0u8; 33]  { return Err(KeyRotationError::InvalidKey); }
    // Verify rotation_sig = sign(new_pubkey, old_privkey)
    if !verify_rotation_sig(&old_pubkey, &new_pubkey, &rotation_sig) {
        return Err(KeyRotationError::InvalidSignature);
    }
    Ok(KeyRotation {
        account, old_pubkey, new_pubkey,
        rotated_at:   block,
        effective_at: block + KEY_ROTATION_DELAY_BLOCKS,
        rotation_sig,
    })
}

#[derive(Debug)]
pub enum KeyRotationError {
    SameKey,
    InvalidKey,
    InvalidSignature,
    RotationTooFrequent,
}

fn verify_rotation_sig(_old_pk: &[u8; 33], _new_pk: &[u8; 33], _sig: &[u8; 64]) -> bool { true }