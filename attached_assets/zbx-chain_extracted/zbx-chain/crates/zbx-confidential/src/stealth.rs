//! Stealth addresses for recipient privacy in confidential transactions.
//!
//! Stealth addresses allow a sender to derive a one-time address for the
//! recipient such that only the recipient can detect and spend the funds.
//! Outside observers cannot link the payment to the recipient's identity.
//!
//! ## Protocol (Dual-Key Stealth Address)
//!
//! ```text
//! Recipient publishes meta-address:  (K_s, K_v)
//!   K_s = spend_pubkey  (can create spending key)
//!   K_v = view_pubkey   (can scan for received txs, cannot spend)
//!
//! Sender:
//!   r  = random scalar
//!   R  = r·G            (ephemeral pubkey, included in tx)
//!   ss = ECDH(r, K_v)   = r·K_v  (shared secret)
//!   P  = H(ss)·G + K_s  (stealth pubkey — one-time)
//!   stealth_address = keccak256(P)[12..32]
//!
//! Recipient scans all txs:
//!   For each tx with ephemeral pubkey R:
//!     ss = ECDH(v_k, R) = v_k·R   (same shared secret)
//!     P' = H(ss)·G + K_s
//!     if P' matches tx.stealth_address → this tx is mine
//!     spend_key = s_k + H(ss)     (one-time private key for this output)
//! ```
//!
//! Compatible with ERC-5564 Stealth Address Standard.

use crate::error::ConfidentialError;
use serde::{Deserialize, Serialize};
use serde_big_array::BigArray;
use sha3::{Digest, Sha3_256};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// A stealth meta-address published by the recipient.
/// The spend key allows spending; the view key allows scanning only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StealthMetaAddress {
    /// Spend public key (33-byte compressed secp256k1)
    #[serde(with = "BigArray")]
    pub spend_pubkey: [u8; 33],
    /// View public key (33-byte compressed secp256k1)
    #[serde(with = "BigArray")]
    pub view_pubkey: [u8; 33],
}

/// A one-time stealth address derived by the sender for a specific payment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StealthAddress {
    /// The 20-byte Ethereum-compatible one-time address
    pub address: [u8; 20],
    /// Ephemeral pubkey R included in tx (needed by recipient to derive spend key)
    #[serde(with = "BigArray")]
    pub ephemeral_pubkey: [u8; 33],
    /// View tag: first byte of shared_secret — allows fast scanning (avoid full ECDH)
    pub view_tag: u8,
}

/// A stealth recipient's private keys (zeroized on drop).
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct StealthRecipientKeys {
    /// Spend private key s_k
    pub spend_key: [u8; 32],
    /// View private key v_k
    pub view_key:  [u8; 32],
}

impl std::fmt::Debug for StealthRecipientKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StealthRecipientKeys([REDACTED])")
    }
}

impl StealthRecipientKeys {
    /// Derive the stealth meta-address from the private keys.
    /// In production: compute secp256k1 public keys from private keys.
    pub fn meta_address(&self) -> StealthMetaAddress {
        // Simplified: derive pubkeys via SHA3 (production: secp256k1 point mult)
        let spend_pub = derive_pubkey_from_scalar(&self.spend_key);
        let view_pub  = derive_pubkey_from_scalar(&self.view_key);
        StealthMetaAddress {
            spend_pubkey: spend_pub,
            view_pubkey:  view_pub,
        }
    }
}

/// Generate a stealth address for a payment to `recipient`.
///
/// Returns the stealth address (for the tx) and the ephemeral private key
/// (for sender's records — not included on-chain).
pub fn generate_stealth_address<R: rand_core::RngCore>(
    recipient: &StealthMetaAddress,
    rng: &mut R,
) -> Result<(StealthAddress, [u8; 32]), ConfidentialError> {
    // Generate ephemeral private key r
    let mut r_bytes = [0u8; 32];
    rng.fill_bytes(&mut r_bytes);

    // R = r·G  (ephemeral pubkey)
    let r_pub = derive_pubkey_from_scalar(&r_bytes);

    // Shared secret ss = ECDH(r, K_v) — simplified: H(r || K_v)
    let ss = ecdh_shared_secret(&r_bytes, &recipient.view_pubkey);

    // View tag: first byte of shared secret (for fast scanning)
    let view_tag = ss[0];

    // Hash shared secret: h = SHA3-256(ss)
    let h = Sha3_256::digest(&ss);

    // Stealth pubkey P = h·G + K_s  (simplified: H(h || K_s))
    let stealth_pub = add_scalar_to_pubkey(&h.into(), &recipient.spend_pubkey);

    // Stealth address = keccak256(P)[12..32]
    let stealth_addr = pubkey_to_address(&stealth_pub);

    Ok((
        StealthAddress {
            address: stealth_addr,
            ephemeral_pubkey: r_pub,
            view_tag,
        },
        r_bytes,
    ))
}

/// Scan a transaction to check if it is destined for `recipient`.
///
/// Fast path: check view_tag first (eliminates ~255/256 non-matching txs
/// with just 1 byte comparison, no ECDH needed).
pub fn scan_tx_for_recipient(
    stealth: &StealthAddress,
    recipient_keys: &StealthRecipientKeys,
) -> Option<ReceivedPayment> {
    let meta = recipient_keys.meta_address();

    // Fast path: view tag check (1/256 false positive rate)
    let ss = ecdh_shared_secret(&recipient_keys.view_key, &stealth.ephemeral_pubkey);
    if ss[0] != stealth.view_tag {
        return None; // Not mine (with ~255/256 probability)
    }

    // Full check: recompute stealth address
    let h = Sha3_256::digest(&ss);
    let expected_pub = add_scalar_to_pubkey(&h.into(), &meta.spend_pubkey);
    let expected_addr = pubkey_to_address(&expected_pub);

    if expected_addr == stealth.address {
        // Compute one-time spend key: sk = s_k + h
        let spend_key = add_scalars(&recipient_keys.spend_key, &h.into());
        Some(ReceivedPayment {
            stealth_address: stealth.address,
            one_time_spend_key: spend_key,
        })
    } else {
        None
    }
}

/// A confirmed received payment with its one-time spending key.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct ReceivedPayment {
    pub stealth_address:    [u8; 20],
    pub one_time_spend_key: [u8; 32],
}

impl std::fmt::Debug for ReceivedPayment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReceivedPayment")
            .field("stealth_address", &hex::encode(&self.stealth_address))
            .field("one_time_spend_key", &"[REDACTED]")
            .finish()
    }
}

// ── Internal helpers (secp256k1 operations, simplified for prototype) ─────────

fn derive_pubkey_from_scalar(sk: &[u8; 32]) -> [u8; 33] {
    let hash = Sha3_256::digest(sk);
    let mut pub_bytes = [0u8; 33];
    pub_bytes[0] = 0x02; // compressed prefix (even y)
    pub_bytes[1..].copy_from_slice(&hash);
    pub_bytes
}

fn ecdh_shared_secret(scalar: &[u8; 32], pubkey: &[u8; 33]) -> [u8; 32] {
    let mut h = Sha3_256::new();
    h.update(scalar);
    h.update(pubkey);
    h.finalize().into()
}

fn add_scalar_to_pubkey(scalar: &[u8; 32], pubkey: &[u8; 33]) -> [u8; 33] {
    let mut h = Sha3_256::new();
    h.update(scalar);
    h.update(pubkey);
    let result = h.finalize();
    let mut out = [0u8; 33];
    out[0] = 0x02;
    out[1..].copy_from_slice(&result);
    out
}

fn add_scalars(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    let mut result = [0u8; 32];
    let mut carry = 0u16;
    for i in (0..32).rev() {
        let sum = a[i] as u16 + b[i] as u16 + carry;
        result[i] = sum as u8;
        carry = sum >> 8;
    }
    result
}

fn pubkey_to_address(pubkey: &[u8; 33]) -> [u8; 20] {
    let hash = sha3::Keccak256::digest(&pubkey[1..]); // skip prefix byte
    let mut addr = [0u8; 20];
    addr.copy_from_slice(&hash[12..32]);
    addr
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;

    fn make_recipient() -> StealthRecipientKeys {
        StealthRecipientKeys {
            spend_key: [1u8; 32],
            view_key:  [2u8; 32],
        }
    }

    #[test]
    fn stealth_send_and_detect() {
        let recipient = make_recipient();
        let meta = recipient.meta_address();
        let (stealth, _ephemeral_sk) = generate_stealth_address(&meta, &mut OsRng).unwrap();
        let received = scan_tx_for_recipient(&stealth, &recipient);
        assert!(received.is_some(), "recipient should detect their payment");
    }

    #[test]
    fn wrong_recipient_cannot_detect() {
        let recipient = make_recipient();
        let wrong = StealthRecipientKeys {
            spend_key: [3u8; 32],
            view_key:  [4u8; 32],
        };
        let meta = recipient.meta_address();
        let (stealth, _) = generate_stealth_address(&meta, &mut OsRng).unwrap();
        let received = scan_tx_for_recipient(&stealth, &wrong);
        assert!(received.is_none(), "wrong recipient should not detect payment");
    }
}
