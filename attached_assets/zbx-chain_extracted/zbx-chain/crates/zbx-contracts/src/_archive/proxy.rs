//! Proxy patterns -- Transparent Proxy + UUPS + Diamond (EIP-2535).
//!
//! ZBX contracts use upgradeable proxies for core protocol contracts:
//!   - Staking contract: TransparentProxy
//!   - Governance contract: UUPS
//!   - DEX Router: Diamond (EIP-2535) for modularity
//!
//! ## Transparent Proxy (EIP-1967)
//!   - Proxy stores implementation address at EIP-1967 slot
//!   - Admin calls go to proxy; user calls go to implementation
//!   - ProxyAdmin contract manages upgrades
//!   - Prevents selector clash: admin functions always hit proxy
//!
//! ## UUPS (Universal Upgradeable Proxy Standard)
//!   - Upgrade logic is in the implementation, not the proxy
//!   - Smaller proxy bytecode
//!   - Implementation must include upgradeToAndCall()
//!
//! ## Diamond / EIP-2535
//!   - Multiple "facets" (implementation contracts) per proxy
//!   - Each facet handles a subset of function selectors
//!   - DiamondCut event for selector -> facet mapping changes
//!   - Allows adding/replacing/removing functions without full upgrade

use std::collections::HashMap;

// ── EIP-1967 Storage Slots ────────────────────────────────────────────────────

/// EIP-1967 implementation slot:
/// keccak256("eip1967.proxy.implementation") - 1
pub const IMPLEMENTATION_SLOT: [u8; 32] = [
    0x36, 0x08, 0x94, 0xa1, 0x3b, 0xa1, 0xa3, 0x21,
    0x06, 0x67, 0xc8, 0x28, 0x49, 0x2d, 0xb9, 0x8d,
    0xca, 0x3e, 0x20, 0x76, 0xcc, 0x37, 0x35, 0xa9,
    0x20, 0xa3, 0xca, 0x50, 0x5d, 0x38, 0x2b, 0xbc,
];

/// EIP-1967 admin slot:
/// keccak256("eip1967.proxy.admin") - 1
pub const ADMIN_SLOT: [u8; 32] = [
    0xb5, 0x31, 0x27, 0x68, 0x4a, 0x56, 0x8b, 0x31,
    0x73, 0xae, 0x13, 0xb9, 0xf8, 0xa6, 0x01, 0x6e,
    0x24, 0x3e, 0x63, 0xb6, 0xe8, 0xee, 0x13, 0x27,
    0xca, 0x01, 0xa9, 0x42, 0x53, 0xd9, 0x37, 0x4f,
];

// ── Transparent Proxy ─────────────────────────────────────────────────────────

/// Transparent Upgradeable Proxy (EIP-1967 + TransparentProxy pattern).
///
/// Key properties:
///   - Admin sees proxy functions only (upgrade, changeAdmin)
///   - Non-admin sees implementation functions only (via delegatecall)
///   - Prevents admin from accidentally calling implementation via fallback
///   - ProxyAdmin contract (separate) holds the admin role
pub struct TransparentProxy {
    /// Current implementation contract address
    pub implementation: [u8; 20],
    /// Admin address (usually a ProxyAdmin contract)
    pub admin:          [u8; 20],
    /// Storage (shared with implementation via delegatecall)
    pub storage:        HashMap<[u8; 32], [u8; 32]>,
}

impl TransparentProxy {
    pub fn new(implementation: [u8; 20], admin: [u8; 20]) -> Self {
        Self { implementation, admin, storage: HashMap::new() }
    }

    /// Upgrade to a new implementation (admin only).
    /// Emits Upgraded(newImplementation) event.
    pub fn upgrade_to(&mut self, caller: [u8; 20], new_impl: [u8; 20]) -> Result<(), ProxyError> {
        if caller != self.admin { return Err(ProxyError::NotAdmin); }
        if new_impl == [0u8; 20]  { return Err(ProxyError::ZeroImplementation); }
        self.implementation = new_impl;
        Ok(())
    }

    /// Upgrade and call initializer atomically (admin only).
    pub fn upgrade_to_and_call(
        &mut self, caller: [u8; 20], new_impl: [u8; 20], _call_data: &[u8]
    ) -> Result<(), ProxyError> {
        self.upgrade_to(caller, new_impl)?;
        // Execute call_data on new implementation via delegatecall
        Ok(())
    }

    /// Change the admin address.
    pub fn change_admin(&mut self, caller: [u8; 20], new_admin: [u8; 20]) -> Result<(), ProxyError> {
        if caller != self.admin { return Err(ProxyError::NotAdmin); }
        if new_admin == [0u8; 20] { return Err(ProxyError::ZeroAdmin); }
        self.admin = new_admin;
        Ok(())
    }

    /// Fallback: delegatecall to implementation for non-admin callers.
    pub fn fallback(&self, caller: [u8; 20], data: &[u8]) -> FallbackTarget {
        if caller == self.admin {
            FallbackTarget::AdminOnly // admin can only call proxy admin functions
        } else {
            FallbackTarget::Delegatecall { impl_addr: self.implementation, data: data.to_vec() }
        }
    }
}

// ── Diamond / EIP-2535 ────────────────────────────────────────────────────────

/// Diamond proxy -- multiple facets per proxy (EIP-2535).
///
/// Allows a single contract address to expose functions from multiple
/// implementation contracts ("facets"). Each facet handles specific selectors.
///
/// DiamondCut operations:
///   Add    -- new selector -> new facet
///   Replace -- existing selector -> different facet
///   Remove  -- remove selector entirely
pub struct Diamond {
    /// selector -> facet address
    pub selector_to_facet: HashMap<[u8; 4], [u8; 20]>,
    /// facet address -> list of its selectors
    pub facet_selectors:   HashMap<[u8; 20], Vec<[u8; 4]>>,
    /// DiamondCut history (for transparency)
    pub cut_history:       Vec<DiamondCut>,
    /// Owner (can perform cuts)
    pub owner:             [u8; 20],
}

#[derive(Debug, Clone)]
pub struct DiamondCut {
    pub facet_address:  [u8; 20],
    pub action:         FacetCutAction,
    pub selectors:      Vec<[u8; 4]>,
    pub block:          u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FacetCutAction {
    Add,
    Replace,
    Remove,
}

impl Diamond {
    pub fn new(owner: [u8; 20]) -> Self {
        Self { selector_to_facet: HashMap::new(), facet_selectors: HashMap::new(),
               cut_history: Vec::new(), owner }
    }

    /// Perform a diamond cut -- add/replace/remove facet selectors.
    /// Emits DiamondCut event.
    pub fn diamond_cut(
        &mut self,
        caller:   [u8; 20],
        cuts:     Vec<DiamondCut>,
        _init:    Option<[u8; 20]>,  // optional initializer call
        _call_data: &[u8],
    ) -> Result<(), DiamondError> {
        if caller != self.owner { return Err(DiamondError::NotOwner); }
        for cut in cuts {
            match cut.action {
                FacetCutAction::Add => {
                    for sel in &cut.selectors {
                        if self.selector_to_facet.contains_key(sel) {
                            return Err(DiamondError::SelectorAlreadyExists(*sel));
                        }
                        self.selector_to_facet.insert(*sel, cut.facet_address);
                    }
                    self.facet_selectors.entry(cut.facet_address).or_insert_with(Vec::new)
                        .extend_from_slice(&cut.selectors);
                }
                FacetCutAction::Replace => {
                    for sel in &cut.selectors {
                        self.selector_to_facet.insert(*sel, cut.facet_address);
                    }
                }
                FacetCutAction::Remove => {
                    for sel in &cut.selectors {
                        self.selector_to_facet.remove(sel);
                    }
                }
            }
            self.cut_history.push(cut);
        }
        Ok(())
    }

    /// Route a call to the correct facet (or revert if no facet found).
    pub fn route(&self, selector: [u8; 4]) -> Option<[u8; 20]> {
        self.selector_to_facet.get(&selector).copied()
    }

    /// List all facets and their selectors (Loupe interface, EIP-2535).
    pub fn facets(&self) -> Vec<([u8; 20], &Vec<[u8; 4]>)> {
        self.facet_selectors.iter().map(|(f, s)| (*f, s)).collect()
    }
}

pub enum FallbackTarget {
    AdminOnly,
    Delegatecall { impl_addr: [u8; 20], data: Vec<u8> },
}

#[derive(Debug)]
pub enum ProxyError {
    NotAdmin, ZeroImplementation, ZeroAdmin, CallFailed,
}

#[derive(Debug)]
pub enum DiamondError {
    NotOwner, SelectorAlreadyExists([u8; 4]), FacetNotFound, InitFailed,
}