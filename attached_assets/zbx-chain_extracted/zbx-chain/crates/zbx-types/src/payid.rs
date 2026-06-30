//! PayID precompile (0x0A) — on-chain registry layout helpers.
//!
//! Lives in `zbx-types` (a leaf crate) so both the ZVM and EVM precompile
//! modules can depend on it without pulling the heavy `zbx-payid` runtime
//! deps (`reqwest`, `tokio`) — and so the production hosts in `zbx-state`
//! can use the exact same slot derivation as the precompile that reads
//! them. `zbx-payid::registry` re-exports the same items for legacy
//! callers.
//!
//! Storage layout under [`PAYID_REGISTRAR_ADDR`]:
//!
//!   * Forward `name → address`: `payid_forward_slot(name)` → 32-byte
//!     word with the resolved address right-aligned in `[12..32]`.
//!     All-zero word means unregistered (caller-observable as
//!     `address(0)`, no revert).
//!
//!   * Reverse `address → name`: `payid_reverse_slot(addr)` → 32-byte
//!     word with the ASCII PayID name left-aligned, zero-padded. Names
//!     are ≤ 32 ASCII chars (per [`validate_payid_name`]) so a single
//!     slot is sufficient.

/// Canonical chain-state address that holds the PayID registry slots
/// the 0x0A precompile reads. Distinct from the precompile address
/// itself (`0x000…000A`); using a separate registrar keeps precompile
/// dispatch stateless and lets governance migrate the registrar
/// without changing the precompile address.
pub const PAYID_REGISTRAR_ADDR: [u8; 20] = {
    let mut a = [0u8; 20];
    a[19] = 0xC0;
    a
};

/// Storage slot for the forward `name → address` mapping under
/// [`PAYID_REGISTRAR_ADDR`]. `keccak256("payid/" || name_ascii)`.
pub fn payid_forward_slot(name: &[u8]) -> [u8; 32] {
    use sha3::{Digest, Keccak256};
    let mut h = Keccak256::new();
    h.update(b"payid/");
    h.update(name);
    h.finalize().into()
}

/// Storage slot for the reverse `address → name` mapping under
/// [`PAYID_REGISTRAR_ADDR`]. `keccak256("payid_rev/" || addr20)`.
pub fn payid_reverse_slot(addr: &[u8; 20]) -> [u8; 32] {
    use sha3::{Digest, Keccak256};
    let mut h = Keccak256::new();
    h.update(b"payid_rev/");
    h.update(addr);
    h.finalize().into()
}

/// Validate the precompile-input PayID name: 3..=32 ASCII bytes from
/// the set `[a-z0-9._-]`. Caller is expected to have already stripped
/// any optional `@zbx` suffix.
pub fn validate_payid_name(name: &[u8]) -> bool {
    let n = name.len();
    if !(3..=32).contains(&n) {
        return false;
    }
    name.iter().all(|&c| {
        c.is_ascii_lowercase()
            || c.is_ascii_digit()
            || c == b'.'
            || c == b'_'
            || c == b'-'
    })
}

/// Read-only PayID registry surface used by the ZVM/EVM 0x0A precompile.
/// Implemented by the production hosts (over real chain state via the
/// slot helpers above) and by lightweight mocks in tests.
pub trait PayIdLookup {
    fn resolve(&self, name: &[u8]) -> Option<[u8; 20]>;
    fn reverse(&self, addr: &[u8; 20]) -> Option<Vec<u8>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_and_reverse_slots_differ_for_same_input() {
        let name = b"alice";
        let addr = [0xAAu8; 20];
        assert_ne!(payid_forward_slot(name), payid_reverse_slot(&addr));
    }

    #[test]
    fn validate_rejects_bad_inputs() {
        assert!(validate_payid_name(b"abc"));
        assert!(validate_payid_name(&[b'a'; 32]));
        assert!(!validate_payid_name(b"ab"));
        assert!(!validate_payid_name(&[b'a'; 33]));
        assert!(!validate_payid_name(b"Alice"));
        assert!(!validate_payid_name(b"alice@zbx"));
        assert!(!validate_payid_name(b"al ce"));
    }

    #[test]
    fn registrar_address_distinct_from_precompile_range() {
        assert!(PAYID_REGISTRAR_ADDR[19] > 0x0F);
    }
}
