//! Admin Access Control -- Ownable + RBAC + Pauser
//!
//! ZBX uses a layered admin model:
//!   SuperAdmin  -- can do everything (held by Foundation multi-sig)
//!   Operator    -- can change chain params, slash validators
//!   Pauser      -- can pause/unpause only
//!   Upgrader    -- can schedule protocol upgrades (timelock required)
//!
//! Ownable pattern:
//!   - owner can transfer ownership to a new address
//!   - owner can renounce ownership (lock admin forever)
//!   - owner can grant / revoke roles to other addresses
//!
//! All admin actions emit AdminEvent and are written to AuditLog.

use std::collections::{HashMap, HashSet};

// ── Admin Roles ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdminRole {
    SuperAdmin,  // Full access -- Foundation multi-sig
    Operator,    // Chain params + validator management
    Pauser,      // pause() / unpause() only
    Upgrader,    // Schedule hard forks (timelock required)
    Treasury,    // Mint/burn ZBX, treasury withdrawals
    Guardian,    // Emergency stop, freeze accounts
}

impl AdminRole {
    pub fn display(&self) -> &'static str {
        match self {
            Self::SuperAdmin => "SuperAdmin",
            Self::Operator   => "Operator",
            Self::Pauser     => "Pauser",
            Self::Upgrader   => "Upgrader",
            Self::Treasury   => "Treasury",
            Self::Guardian   => "Guardian",
        }
    }
}

// ── Ownable ───────────────────────────────────────────────────────────────────

/// Ownable -- single-owner admin contract pattern.
/// The owner is the Foundation multi-sig (3-of-5 threshold).
pub struct Ownable {
    /// Current owner address (Foundation multi-sig)
    pub owner:            [u8; 20],
    /// Pending ownership transfer (two-step transfer)
    pub pending_owner:    Option<[u8; 20]>,
    /// Role assignments: address -> set of AdminRoles
    pub role_members:     HashMap<[u8; 20], HashSet<AdminRole>>,
    /// Role admin: who can grant/revoke each role
    pub role_admins:      HashMap<AdminRole, AdminRole>,
}

impl Ownable {
    pub fn new(owner: [u8; 20]) -> Self {
        let mut role_members = HashMap::new();
        let mut roles = HashSet::new();
        roles.insert(AdminRole::SuperAdmin);
        role_members.insert(owner, roles);
        let mut role_admins = HashMap::new();
        role_admins.insert(AdminRole::Operator,  AdminRole::SuperAdmin);
        role_admins.insert(AdminRole::Pauser,    AdminRole::SuperAdmin);
        role_admins.insert(AdminRole::Upgrader,  AdminRole::SuperAdmin);
        role_admins.insert(AdminRole::Treasury,  AdminRole::SuperAdmin);
        role_admins.insert(AdminRole::Guardian,  AdminRole::SuperAdmin);
        Self { owner, pending_owner: None, role_members, role_admins }
    }

    /// Initiate a two-step ownership transfer.
    /// New owner must call accept_ownership() to complete.
    pub fn transfer_ownership(&mut self, caller: [u8; 20], new_owner: [u8; 20]) -> Result<AdminEvent, AdminError> {
        self.only_owner(caller)?;
        if new_owner == [0u8; 20] { return Err(AdminError::ZeroAddress); }
        self.pending_owner = Some(new_owner);
        Ok(AdminEvent::OwnershipTransferInitiated { from: caller, to: new_owner })
    }

    /// Accept pending ownership transfer. Must be called by pending_owner.
    pub fn accept_ownership(&mut self, caller: [u8; 20]) -> Result<AdminEvent, AdminError> {
        match self.pending_owner {
            Some(po) if po == caller => {
                let old = self.owner;
                // Grant SuperAdmin to new owner
                self.role_members.entry(caller).or_insert_with(HashSet::new)
                    .insert(AdminRole::SuperAdmin);
                // Revoke SuperAdmin from old owner (optional, configurable)
                self.owner = caller;
                self.pending_owner = None;
                Ok(AdminEvent::OwnershipTransferred { from: old, to: caller })
            }
            _ => Err(AdminError::NotPendingOwner),
        }
    }

    /// Renounce ownership -- permanently lock admin (IRREVERSIBLE).
    /// Sets owner to zero address. Used in progressive decentralization.
    pub fn renounce_ownership(&mut self, caller: [u8; 20]) -> Result<AdminEvent, AdminError> {
        self.only_owner(caller)?;
        let old = self.owner;
        self.owner = [0u8; 20]; // Zero address = no owner
        self.pending_owner = None;
        Ok(AdminEvent::OwnershipRenounced { prev_owner: old })
    }

    /// Grant a role to an address.
    pub fn grant_role(&mut self, caller: [u8; 20], role: AdminRole, account: [u8; 20]) -> Result<AdminEvent, AdminError> {
        self.check_role_admin(caller, role)?;
        if account == [0u8; 20] { return Err(AdminError::ZeroAddress); }
        self.role_members.entry(account).or_insert_with(HashSet::new).insert(role);
        Ok(AdminEvent::RoleGranted { role, account, by: caller })
    }

    /// Revoke a role from an address.
    pub fn revoke_role(&mut self, caller: [u8; 20], role: AdminRole, account: [u8; 20]) -> Result<AdminEvent, AdminError> {
        self.check_role_admin(caller, role)?;
        if let Some(roles) = self.role_members.get_mut(&account) {
            roles.remove(&role);
        }
        Ok(AdminEvent::RoleRevoked { role, account, by: caller })
    }

    /// Revoke access: remove ALL roles from an account.
    pub fn revoke_access(&mut self, caller: [u8; 20], account: [u8; 20]) -> Result<AdminEvent, AdminError> {
        self.only_owner(caller)?;
        self.role_members.remove(&account);
        Ok(AdminEvent::AccessRevoked { account, by: caller })
    }

    /// Check if an account has a specific role.
    pub fn has_role(&self, account: [u8; 20], role: AdminRole) -> bool {
        self.role_members.get(&account).map(|r| r.contains(&role)).unwrap_or(false)
    }

    fn only_owner(&self, caller: [u8; 20]) -> Result<(), AdminError> {
        if caller != self.owner { return Err(AdminError::NotOwner); }
        Ok(())
    }

    fn check_role_admin(&self, caller: [u8; 20], role: AdminRole) -> Result<(), AdminError> {
        let required_admin = self.role_admins.get(&role).copied().unwrap_or(AdminRole::SuperAdmin);
        if !self.has_role(caller, required_admin) && caller != self.owner {
            return Err(AdminError::Unauthorized { required: required_admin });
        }
        Ok(())
    }
}

// ── Admin Events ──────────────────────────────────────────────────────────────

/// All admin actions produce an AdminEvent that is emitted on-chain
/// and appended to the AuditLog.
#[derive(Debug, Clone)]
pub enum AdminEvent {
    // Ownership
    OwnershipTransferInitiated { from: [u8; 20], to: [u8; 20] },
    OwnershipTransferred       { from: [u8; 20], to: [u8; 20] },
    OwnershipRenounced         { prev_owner: [u8; 20] },
    // Roles
    RoleGranted  { role: AdminRole, account: [u8; 20], by: [u8; 20] },
    RoleRevoked  { role: AdminRole, account: [u8; 20], by: [u8; 20] },
    AccessRevoked { account: [u8; 20], by: [u8; 20] },
    // Emergency
    Paused       { by: [u8; 20], block: u64 },
    Unpaused     { by: [u8; 20], block: u64 },
    AccountFrozen { account: [u8; 20], by: [u8; 20] },
    AccountBlacklisted { account: [u8; 20], by: [u8; 20] },
    CircuitBreakerTripped { reason: String, block: u64 },
    // Chain params
    BaseFeeChanged    { old: u64, new: u64, by: [u8; 20] },
    GasLimitChanged   { old: u64, new: u64, by: [u8; 20] },
    ConfigChanged     { key: String, old: String, new: String, by: [u8; 20] },
    // Treasury
    FeeDistributed    { amount: u128, recipients: u32, block: u64 },
    ProtocolRevenue   { amount: u128, source: String, block: u64 },
    // Validators
    ValidatorSetOverridden { count: u32, by: [u8; 20] },
    ValidatorSlotsChanged  { old: u32, new: u32, by: [u8; 20] },
    ValidatorWhitelisted   { validator: [u8; 20], by: [u8; 20] },
    // Upgrades
    HardForkScheduled { fork_name: String, activation_block: u64, by: [u8; 20] },
    UpgradeVotePassed { proposal_id: u64, yes: u128, no: u128 },
    // Keys
    AdminKeyRotated   { old_pubkey: [u8; 33], new_pubkey: [u8; 33], by: [u8; 20] },
}

#[derive(Debug)]
pub enum AdminError {
    NotOwner,
    NotPendingOwner,
    ZeroAddress,
    Unauthorized { required: AdminRole },
    AlreadyPaused,
    NotPaused,
    TimelockNotExpired,
}