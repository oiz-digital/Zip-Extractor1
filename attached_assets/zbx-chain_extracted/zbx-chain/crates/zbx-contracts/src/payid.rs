//! PayID registry contract — maps human-readable IDs to chain addresses.

use std::collections::HashMap;
use zbx_types::address::Address;

/// Canonical PayID format: user$domain (e.g. "alice$zebvix.io")
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct PayId(pub String);

impl std::fmt::Display for PayId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// In-memory PayID registry (mirrors the on-chain contract state).
#[derive(Debug, Default)]
pub struct PayIdRegistry {
    records: HashMap<PayId, Address>,
    owners:  HashMap<PayId, Address>,
}

impl PayIdRegistry {
    pub fn new() -> Self { Self::default() }

    /// Register a new PayID mapping.
    ///
    /// ## PAY-01 fix (2026-05-16) — format validation
    ///
    /// PayIDs MUST follow the `user$domain` format (e.g. `alice$zebvix.io`)
    /// defined in the PayID protocol spec.  Without this check, any arbitrary
    /// string (e.g. `"alice"`) can be registered.  Such records would be
    /// silently unresolvable by any standards-compliant PayID resolver while
    /// still blocking the `user$domain` form from being registered later.
    ///
    /// The check requires exactly one `$` separator that is neither the first
    /// nor the last character (i.e. both the user and domain parts are
    /// non-empty).
    pub fn register(&mut self, pay_id: PayId, owner: Address, target: Address) -> Result<(), &'static str> {
        // PAY-01: enforce `user$domain` format.
        match pay_id.0.find('$') {
            None => return Err("PayID must be in user$domain format (missing '$')"),
            Some(0) => return Err("PayID user part must not be empty"),
            Some(pos) if pos == pay_id.0.len() - 1 => {
                return Err("PayID domain part must not be empty");
            }
            _ => {}
        }
        if self.records.contains_key(&pay_id) {
            return Err("PayID already registered");
        }
        self.records.insert(pay_id.clone(), target);
        self.owners.insert(pay_id, owner);
        Ok(())
    }

    pub fn resolve(&self, pay_id: &PayId) -> Option<Address> {
        self.records.get(pay_id).copied()
    }

    pub fn update(&mut self, pay_id: &PayId, caller: &Address, new_target: Address) -> Result<(), &'static str> {
        if self.owners.get(pay_id) != Some(caller) {
            return Err("not owner");
        }
        *self.records.get_mut(pay_id).ok_or("not found")? = new_target;
        Ok(())
    }
}
