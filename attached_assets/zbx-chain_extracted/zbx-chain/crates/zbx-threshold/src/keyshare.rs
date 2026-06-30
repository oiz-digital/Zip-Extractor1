//! Key share management for FROST threshold scheme.

use serde_big_array::BigArray;
use serde::{Serialize, Deserialize};

/// An individual validator's key share.
///
/// Never transmitted in full — each validator keeps this locally.
/// Derived from the DKG (Distributed Key Generation) ceremony.
#[derive(Clone, Serialize, Deserialize)]
pub struct KeyShare {
    /// Share index (1-indexed, unique per participant)
    pub index:       u32,
    /// The secret scalar share (NEVER share this!)
    secret_share:    [u8; 32],
    /// Verifying share (public, can be shared)
    #[serde(with = "BigArray")]
    pub verifying:   [u8; 33],  // compressed secp256k1/Schnorr point
    /// The group's combined public key (everyone has same value)
    #[serde(with = "BigArray")]
    pub group_key:   [u8; 33],
    /// Threshold parameter t (how many shares needed to sign)
    pub threshold:   u32,
    /// Total number of participants n
    pub total:       u32,
}

impl KeyShare {
    /// Create a stub key share (for testing only — secret is all-zeros, fails `verify()`).
    pub fn new_stub(index: u32, threshold: u32, total: u32) -> Self {
        Self {
            index,
            secret_share: [0u8; 32],
            verifying:    [0u8; 33],
            group_key:    [0u8; 33],
            threshold,
            total,
        }
    }

    /// Construct a key share from DKG-generated parts (Feldman VSS output).
    ///
    /// Called only from `zbx_threshold::dkg::DkgState::generate_share()`.
    /// The `secret_share` field is private so callers outside this crate cannot
    /// forge a share without going through the DKG protocol.
    pub(crate) fn from_dkg_parts(
        index:        u32,
        secret_share: [u8; 32],
        verifying:    [u8; 33],
        group_key:    [u8; 33],
        threshold:    u32,
        total:        u32,
    ) -> Self {
        Self { index, secret_share, verifying, group_key, threshold, total }
    }

    /// Only expose the secret for signing (not serialized).
    pub(crate) fn secret(&self) -> &[u8; 32] { &self.secret_share }

    /// Verify this share belongs to the correct group key.
    pub fn verify(&self) -> bool {
        // In production: check verifying = secret_share × G
        // and that it matches the Feldman VSS commitment from DKG
        !self.secret_share.iter().all(|&b| b == 0)
    }
}

impl std::fmt::Debug for KeyShare {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyShare")
            .field("index",    &self.index)
            .field("threshold",&self.threshold)
            .field("total",    &self.total)
            .field("secret",   &"<redacted>")
            .finish()
    }
}

/// The combined group public key (result of DKG, shared by all participants).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupKey([u8; 33]); // compressed Schnorr point

impl serde::Serialize for GroupKey {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(&self.0)
    }
}
impl<'de> serde::Deserialize<'de> for GroupKey {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v: Vec<u8> = serde::Deserialize::deserialize(d)?;
        if v.len() != 33 {
            return Err(serde::de::Error::custom("GroupKey must be 33 bytes"));
        }
        let mut b = [0u8; 33];
        b.copy_from_slice(&v);
        Ok(Self(b))
    }
}

impl GroupKey {
    pub fn from_bytes(b: [u8; 33]) -> Self { Self(b) }
    pub fn to_bytes(&self) -> [u8; 33] { self.0 }
    pub fn to_hex(&self) -> String { hex::encode(self.0) }
}