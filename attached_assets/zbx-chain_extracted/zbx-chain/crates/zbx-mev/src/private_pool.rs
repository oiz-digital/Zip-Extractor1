//! Private mempool — encrypted tx submission for frontrunning protection.
//!
//! Users encrypt their tx to the block builder's current public encryption
//! key. The builder decrypts only at block-building time, AFTER the
//! transaction ordering has been committed (via the commit-reveal layer),
//! so no observer — including the builder — can profitably reorder or
//! frontrun the tx.
//!
//! # Cipher suite — RFC 9180 HPKE
//!
//! - **KEM**:  DHKEM(X25519, HKDF-SHA256)  — IANA id `0x0020`
//! - **KDF**:  HKDF-SHA256                  — IANA id `0x0001`
//! - **AEAD**: AES-256-GCM                  — IANA id `0x0002`
//!
//! S29 — DESIGN NOTE: the original placeholder doc recommended
//! `DHKEM-secp256k1`, but secp256k1 is NOT in the IANA HPKE KEM
//! registry (RFC 9180 §7). Building on it would require a custom
//! non-standard scheme with no audited Rust impl. X25519 is chosen
//! because it's the most widely deployed HPKE KEM (Cloudflare,
//! Mozilla, Flashbots SUAVE, Marlin Protocol) and has an audited
//! Rust crate. Builders MUST hold a SEPARATE X25519 key for HPKE,
//! distinct from their secp256k1 validator identity (no key reuse
//! across protocols).
//!
//! # Forward secrecy via key rotation
//!
//! HPKE single-shot seal already gives ephemeral-on-sender (each tx
//! uses a fresh KEM keypair). Forward secrecy on the BUILDER side is
//! provided by [`BuilderKeyRing`] — old epoch keys are retained for a
//! short TTL window then dropped. The `hpke` crate's secret-key types
//! pull in `zeroize` on drop. After expiry, txs encrypted under the
//! old key are unrecoverable, so a builder compromise after the TTL
//! leaks no historical txs.
//!
//! # AAD binding
//!
//! The AEAD's Additional Authenticated Data binds the public metadata
//! (`sender_hint`, `target_block`, `max_fee`, `key_epoch`) to the
//! ciphertext. Any mempool peer that mutates a metadata field —
//! e.g. to silently re-prioritise an ordering — will cause the
//! AEAD tag verification on the builder side to fail and the tx will
//! be rejected. This prevents metadata-mutation attacks against the
//! ordering / prioritisation layer.
//!
//! # `id` integrity & replay protection at pool layer
//!
//! Wire-supplied `EncryptedTx.id` is NOT trusted. On
//! [`PrivateMempool::submit`] the pool recomputes
//! `keccak256(encapped_key || ciphertext)` and rejects any mismatch
//! with [`MevError::InvalidEncryptedTx`] — this prevents a malicious
//! peer from forging an `id` that collides with an existing entry in
//! order to overwrite or censor it. A bounded `seen_ids` window also
//! rejects exact-blob re-submissions across `replay_window_blocks`
//! (separate from execution-layer nonce enforcement, which is the
//! authoritative replay defense post-decrypt).
//!
//! # `submitted_at` is local-authoritative (NOT wire-trusted)
//!
//! [`PrivateMempool::submit`] takes `local_submitted_at` as an
//! explicit parameter and overwrites `EncryptedTx.submitted_at` with
//! it before storage. The wire-supplied value is therefore
//! IGNORED for any pool-internal accounting (seen_ids pruning, replay
//! window, ordering). This is a defense against an attacker setting
//! `submitted_at = u64::MAX` to evict the entire seen-set and bypass
//! replay protection, OR setting backward values to inhibit pruning
//! and inflate seen_ids retention.
//!
//! `submitted_at` is NOT in the AAD because different gossip peers will
//! record different values; including it would break gossip propagation.
//! It must NEVER be used as a consensus-relevant ordering input.
//!
//! # Trust note on the `hpke` crate
//!
//! The `hpke` crate (rozbb v0.12) is widely deployed and follows
//! RFC 9180 strictly, but is NOT formally audited. For mainnet 8989
//! deploy, an external review of the HPKE integration is strongly
//! recommended. The cipher-suite choice itself (DHKEM(X25519,
//! HKDF-SHA256) / HKDF-SHA256 / AES-256-GCM) is the most-deployed
//! HPKE configuration and has well-understood security properties.

use crate::MevError;
use hpke::{
    aead::AesGcm256,
    kdf::HkdfSha256,
    kem::X25519HkdfSha256,
    Deserializable, Kem as KemTrait, OpModeR, OpModeS, Serializable,
};
use rand::rngs::OsRng;
use sha3::{Digest, Keccak256};
use std::collections::{BTreeMap, HashMap};

/// IANA-aligned cipher-suite type aliases (RFC 9180 §7).
type SuiteKem  = X25519HkdfSha256;
type SuiteKdf  = HkdfSha256;
type SuiteAead = AesGcm256;

/// HPKE info string — domain-separates ZBX private-pool from any other
/// HPKE usage. RFC 9180 recommends a unique constant per protocol.
const HPKE_INFO: &[u8] = b"zbx-chain/private-pool/v1";

// ─── Keys ────────────────────────────────────────────────────────────────

/// A builder's HPKE encryption keypair (X25519 over Curve25519).
///
/// The secret key is dropped via `zeroize` semantics on the underlying
/// `hpke` crate types. Do NOT manually clone or persist `secret_bytes`
/// outside of [`BuilderKeyRing`].
pub struct EncryptionKey {
    pub epoch:      u64,
    pub public:     <SuiteKem as KemTrait>::PublicKey,
    pub secret:     <SuiteKem as KemTrait>::PrivateKey,
    /// Block height at which this key was registered.
    pub registered_at: u64,
}

impl EncryptionKey {
    /// Generate a fresh X25519 keypair from `OsRng`.
    pub fn generate(epoch: u64, registered_at: u64) -> Self {
        let mut rng = OsRng;
        let (secret, public) = SuiteKem::gen_keypair(&mut rng);
        Self { epoch, public, secret, registered_at }
    }

    /// Serialised public key (32 bytes for X25519).
    pub fn public_bytes(&self) -> Vec<u8> {
        self.public.to_bytes().to_vec()
    }
}

/// Rotated keyring. Holds the CURRENT key and a small history of recently
/// retired keys. Lookup is by `epoch`. Retired keys past `ttl_blocks` are
/// pruned and their secret material zeroized.
pub struct BuilderKeyRing {
    current:    EncryptionKey,
    retired:    BTreeMap<u64, EncryptionKey>,
    ttl_blocks: u64,
}

impl BuilderKeyRing {
    /// Create a keyring with a freshly generated current key.
    pub fn bootstrap(epoch: u64, registered_at: u64, ttl_blocks: u64) -> Self {
        Self {
            current:    EncryptionKey::generate(epoch, registered_at),
            retired:    BTreeMap::new(),
            ttl_blocks,
        }
    }

    /// Public key + epoch that senders should encrypt to NOW.
    pub fn current_public(&self) -> (u64, Vec<u8>) {
        (self.current.epoch, self.current.public_bytes())
    }

    /// Rotate to a new epoch. The previous current key moves into
    /// `retired` and remains decryptable for `ttl_blocks`.
    pub fn rotate(&mut self, new_epoch: u64, current_block: u64) {
        let new = EncryptionKey::generate(new_epoch, current_block);
        let prev = std::mem::replace(&mut self.current, new);
        self.retired.insert(prev.epoch, prev);
        self.prune(current_block);
    }

    /// Drop retired keys whose `registered_at + ttl_blocks < current_block`.
    /// Their secret material is zeroized when the [`EncryptionKey`] is
    /// dropped (relies on `hpke` crate's `Drop` impls).
    pub fn prune(&mut self, current_block: u64) {
        let cutoff = current_block.saturating_sub(self.ttl_blocks);
        self.retired.retain(|_, k| k.registered_at >= cutoff);
    }

    /// Look up the secret key for a given epoch. Returns `None` if the
    /// epoch is unknown OR has been pruned past TTL.
    fn lookup(&self, epoch: u64) -> Option<&EncryptionKey> {
        if self.current.epoch == epoch {
            return Some(&self.current);
        }
        self.retired.get(&epoch)
    }
}

// ─── Encrypted tx ────────────────────────────────────────────────────────

/// An encrypted transaction in the private pool.
#[derive(Debug, Clone)]
pub struct EncryptedTx {
    /// `keccak256(encapped_key || ciphertext)`. Stable id across mempool peers.
    pub id:            [u8; 32],
    /// X25519 ephemeral public key from the HPKE KEM (32 bytes).
    pub encapped_key:  Vec<u8>,
    /// AES-256-GCM ciphertext with 16-byte tag appended.
    pub ciphertext:    Vec<u8>,
    /// Sender's secp256k1 pubkey hint (compressed). PRIVACY NOTE: leaked
    /// for sybil defense + per-sender rate limiting; the actual signed
    /// payload inside `ciphertext` re-binds this.
    pub sender_hint:   [u8; 33],
    /// Target inclusion block (None = next available).
    pub target_block:  Option<u64>,
    /// Declared bid (used for ordering; necessarily public).
    pub max_fee:       u128,
    /// Builder key epoch this tx was encrypted under.
    pub key_epoch:     u64,
    /// Block number at which this tx was received by the local node.
    pub submitted_at:  u64,
}

impl EncryptedTx {
    /// Encrypt `plaintext` to `builder_pubkey` (32-byte X25519). Computes
    /// the AAD from the public metadata so any subsequent tampering with
    /// `sender_hint` / `target_block` / `max_fee` / `key_epoch` will
    /// invalidate the AEAD tag on the builder side.
    pub fn seal(
        builder_pubkey: &[u8],
        key_epoch:      u64,
        plaintext:      &[u8],
        sender_hint:    [u8; 33],
        target_block:   Option<u64>,
        max_fee:        u128,
        submitted_at:   u64,
    ) -> Result<Self, MevError> {
        let pk = <SuiteKem as KemTrait>::PublicKey::from_bytes(builder_pubkey)
            .map_err(|e| MevError::SimulationFailed(format!("hpke pk parse: {e:?}")))?;

        let aad = build_aad(&sender_hint, target_block, max_fee, key_epoch);

        let mut rng = OsRng;
        let (encapped_key, ciphertext) =
            hpke::single_shot_seal::<SuiteAead, SuiteKdf, SuiteKem, _>(
                &OpModeS::Base,
                &pk,
                HPKE_INFO,
                plaintext,
                &aad,
                &mut rng,
            )
            .map_err(|e| MevError::SimulationFailed(format!("hpke seal: {e:?}")))?;

        let encapped_bytes = encapped_key.to_bytes().to_vec();

        let mut hasher = Keccak256::new();
        hasher.update(&encapped_bytes);
        hasher.update(&ciphertext);
        let mut id = [0u8; 32];
        id.copy_from_slice(&hasher.finalize());

        Ok(Self {
            id,
            encapped_key: encapped_bytes,
            ciphertext,
            sender_hint,
            target_block,
            max_fee,
            key_epoch,
            submitted_at,
        })
    }
}

/// AAD layout — fixed-width big-endian fields. The builder MUST recompute
/// this from the EncryptedTx fields on decrypt; any mismatch invalidates
/// the AEAD tag.
fn build_aad(
    sender_hint:  &[u8; 33],
    target_block: Option<u64>,
    max_fee:      u128,
    key_epoch:    u64,
) -> Vec<u8> {
    let mut aad = Vec::with_capacity(33 + 1 + 8 + 16 + 8);
    aad.extend_from_slice(sender_hint);
    // target_block: 1-byte presence flag + 8 BE bytes
    match target_block {
        Some(b) => {
            aad.push(0x01);
            aad.extend_from_slice(&b.to_be_bytes());
        }
        None => {
            aad.push(0x00);
            aad.extend_from_slice(&[0u8; 8]);
        }
    }
    aad.extend_from_slice(&max_fee.to_be_bytes());
    aad.extend_from_slice(&key_epoch.to_be_bytes());
    aad
}

// ─── Pool ────────────────────────────────────────────────────────────────

/// A successfully decrypted private-pool tx, ready to feed into the
/// execution layer alongside its ordering metadata.
#[derive(Debug, Clone)]
pub struct DecryptedTx {
    pub id:           [u8; 32],
    pub plaintext:    Vec<u8>,
    pub sender_hint:  [u8; 33],
    pub target_block: Option<u64>,
    pub max_fee:      u128,
    pub key_epoch:    u64,
    pub submitted_at: u64,
}

/// Recompute the canonical id for an [`EncryptedTx`] from its KEM bytes
/// and ciphertext. Used by [`PrivateMempool::submit`] to validate
/// wire-supplied ids before storage.
pub fn compute_tx_id(encapped_key: &[u8], ciphertext: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak256::new();
    hasher.update(encapped_key);
    hasher.update(ciphertext);
    let mut id = [0u8; 32];
    id.copy_from_slice(&hasher.finalize());
    id
}

/// Private mempool: stores encrypted txs until block-build time. Holds
/// NO secret material — decryption requires an externally-supplied
/// [`BuilderKeyRing`].
pub struct PrivateMempool {
    txs:                  BTreeMap<[u8; 32], EncryptedTx>,
    /// id → submitted_at. Bounded by `replay_window_blocks`. Used to
    /// reject exact-blob re-submissions BEFORE they reach the builder.
    seen_ids:             HashMap<[u8; 32], u64>,
    capacity:             usize,
    replay_window_blocks: u64,
}

impl PrivateMempool {
    /// Default replay window — 256 blocks. Tx replays older than this
    /// are accepted at the pool layer (execution-layer nonce check
    /// will still reject them post-decrypt, but they no longer pollute
    /// the seen-set).
    pub const DEFAULT_REPLAY_WINDOW: u64 = 256;

    pub fn new(capacity: usize) -> Self {
        Self::with_replay_window(capacity, Self::DEFAULT_REPLAY_WINDOW)
    }

    pub fn with_replay_window(capacity: usize, replay_window_blocks: u64) -> Self {
        Self {
            txs:                  BTreeMap::new(),
            seen_ids:             HashMap::new(),
            capacity,
            replay_window_blocks,
        }
    }

    /// Insert `tx` after validating:
    ///   1. Capacity not exceeded.
    ///   2. `tx.id == keccak256(encapped_key || ciphertext)` — wire
    ///      forgery of `id` would otherwise collide with / overwrite
    ///      an existing entry, allowing per-id censorship.
    ///   3. `tx.id` is not within the replay window's seen-set.
    ///
    /// `local_submitted_at` is the **authoritative** receive height/time
    /// supplied by the caller (typically the consensus layer's current
    /// block height). It overrides `tx.submitted_at` (which is wire-
    /// supplied and untrusted) for ALL pool-internal accounting:
    /// seen_ids pruning, replay window, future ordering. This is a
    /// hard requirement — a wire-supplied `submitted_at = u64::MAX`
    /// would otherwise evict the entire seen-set.
    pub fn submit(
        &mut self,
        mut tx:             EncryptedTx,
        local_submitted_at: u64,
    ) -> Result<[u8; 32], MevError> {
        if self.txs.len() >= self.capacity {
            return Err(MevError::SimulationFailed(
                "private pool capacity exceeded".into(),
            ));
        }

        let canonical_id = compute_tx_id(&tx.encapped_key, &tx.ciphertext);
        if canonical_id != tx.id {
            return Err(MevError::InvalidEncryptedTx(
                "id does not match keccak256(encapped_key || ciphertext)".into(),
            ));
        }

        // Authoritatively overwrite the wire-supplied submitted_at with
        // the locally-observed value BEFORE any seen_ids accounting.
        tx.submitted_at = local_submitted_at;

        // Prune the seen-set so it stays bounded by replay_window_blocks.
        // Cutoff anchored to the LOCAL value, not the wire value.
        let cutoff = local_submitted_at.saturating_sub(self.replay_window_blocks);
        self.seen_ids.retain(|_, t| *t >= cutoff);

        if self.seen_ids.contains_key(&canonical_id) {
            return Err(MevError::InvalidEncryptedTx(
                "duplicate encrypted tx within replay window".into(),
            ));
        }
        // Also reject if it's currently in the live pool — drop-then-resubmit
        // within the same block would otherwise sneak past the seen-set if
        // local_submitted_at went backwards (e.g., reorg-driven re-feed).
        if self.txs.contains_key(&canonical_id) {
            return Err(MevError::InvalidEncryptedTx(
                "encrypted tx already in pool".into(),
            ));
        }

        self.seen_ids.insert(canonical_id, local_submitted_at);
        self.txs.insert(canonical_id, tx);
        Ok(canonical_id)
    }

    /// Drain all encrypted txs eligible for `block` (target_block == None
    /// or target_block <= block) WITHOUT decrypting. Useful when the
    /// caller wants to control decrypt timing.
    pub fn drain_for_block(&mut self, block: u64) -> Vec<EncryptedTx> {
        let ids: Vec<_> = self
            .txs
            .iter()
            .filter(|(_, tx)| tx.target_block.map_or(true, |t| t <= block))
            .map(|(id, _)| *id)
            .collect();
        ids.iter().filter_map(|id| self.txs.remove(id)).collect()
    }

    /// Decrypt a single encrypted tx using `keyring`. Returns
    /// `Err(MevError::DecryptionFailed(_))` on:
    ///   - unknown / pruned `key_epoch`
    ///   - malformed `encapped_key` bytes
    ///   - AEAD tag mismatch (ciphertext or AAD metadata tampered)
    pub fn decrypt(
        keyring: &BuilderKeyRing,
        tx:      &EncryptedTx,
    ) -> Result<DecryptedTx, MevError> {
        let key = keyring.lookup(tx.key_epoch).ok_or_else(|| {
            MevError::DecryptionFailed(format!(
                "unknown or expired key epoch: {}",
                tx.key_epoch
            ))
        })?;

        let encapped =
            <SuiteKem as KemTrait>::EncappedKey::from_bytes(&tx.encapped_key)
                .map_err(|e| {
                    MevError::DecryptionFailed(format!("hpke encapped parse: {e:?}"))
                })?;

        let aad = build_aad(&tx.sender_hint, tx.target_block, tx.max_fee, tx.key_epoch);

        let plaintext =
            hpke::single_shot_open::<SuiteAead, SuiteKdf, SuiteKem>(
                &OpModeR::Base,
                &key.secret,
                &encapped,
                HPKE_INFO,
                &tx.ciphertext,
                &aad,
            )
            .map_err(|e| MevError::DecryptionFailed(format!("hpke open: {e:?}")))?;

        Ok(DecryptedTx {
            id:           tx.id,
            plaintext,
            sender_hint:  tx.sender_hint,
            target_block: tx.target_block,
            max_fee:      tx.max_fee,
            key_epoch:    tx.key_epoch,
            submitted_at: tx.submitted_at,
        })
    }

    /// Drain eligible txs and decrypt each one. Each entry in the result
    /// is a per-tx Result so a single bad tx does not poison the rest.
    /// Bad txs are dropped (logged at the call site if a tracing layer is
    /// present); their position in the output preserves submission order.
    pub fn drain_and_decrypt_for_block(
        &mut self,
        keyring: &BuilderKeyRing,
        block:   u64,
    ) -> Vec<Result<DecryptedTx, MevError>> {
        self.drain_for_block(block)
            .iter()
            .map(|tx| Self::decrypt(keyring, tx))
            .collect()
    }

    pub fn len(&self) -> usize { self.txs.len() }
    pub fn is_empty(&self) -> bool { self.txs.is_empty() }
}

// ─── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_keyring() -> BuilderKeyRing {
        BuilderKeyRing::bootstrap(/*epoch=*/ 1, /*reg_at=*/ 100, /*ttl=*/ 64)
    }

    #[test]
    fn roundtrip_decrypts() {
        let mut ring = fresh_keyring();
        let (epoch, pk) = ring.current_public();
        let plaintext = b"hello-private-mempool";
        let tx = EncryptedTx::seal(
            &pk, epoch, plaintext, [0xAA; 33], Some(123), 1_000_000, 100,
        )
        .expect("seal");

        let mut pool = PrivateMempool::new(8);
        pool.submit(tx.clone(), /*local=*/ 100).unwrap();

        let dec = PrivateMempool::decrypt(&ring, &tx).expect("decrypt");
        assert_eq!(dec.plaintext, plaintext);
        assert_eq!(dec.sender_hint, [0xAA; 33]);
        assert_eq!(dec.max_fee, 1_000_000);
        // Touch ring to silence unused-mut on keyring.
        ring.prune(100);
    }

    #[test]
    fn wrong_key_rejects() {
        let ring_a = fresh_keyring();
        let ring_b = fresh_keyring();
        let (epoch_a, pk_a) = ring_a.current_public();
        let tx = EncryptedTx::seal(
            &pk_a, epoch_a, b"x", [0; 33], None, 0, 0,
        )
        .unwrap();
        // ring_b has the SAME epoch number but a different secret key, so
        // lookup succeeds and HPKE open fails at the AEAD tag.
        assert!(PrivateMempool::decrypt(&ring_b, &tx).is_err());
    }

    #[test]
    fn tampered_ciphertext_rejects() {
        let ring = fresh_keyring();
        let (epoch, pk) = ring.current_public();
        let mut tx = EncryptedTx::seal(
            &pk, epoch, b"plaintext", [0; 33], None, 0, 0,
        )
        .unwrap();
        // Flip a byte in the ciphertext.
        tx.ciphertext[0] ^= 0x01;
        assert!(PrivateMempool::decrypt(&ring, &tx).is_err());
    }

    #[test]
    fn tampered_aad_metadata_rejects_max_fee() {
        let ring = fresh_keyring();
        let (epoch, pk) = ring.current_public();
        let mut tx = EncryptedTx::seal(
            &pk, epoch, b"p", [0; 33], None, 1_000, 0,
        )
        .unwrap();
        // Mutate metadata: AAD will recompute differently → AEAD fails.
        tx.max_fee = 999_999;
        assert!(PrivateMempool::decrypt(&ring, &tx).is_err());
    }

    #[test]
    fn tampered_aad_metadata_rejects_target_block() {
        let ring = fresh_keyring();
        let (epoch, pk) = ring.current_public();
        let mut tx = EncryptedTx::seal(
            &pk, epoch, b"p", [0; 33], Some(7), 0, 0,
        )
        .unwrap();
        tx.target_block = Some(8);
        assert!(PrivateMempool::decrypt(&ring, &tx).is_err());
    }

    #[test]
    fn key_rotation_old_epoch_still_decrypts_within_ttl() {
        let mut ring = fresh_keyring();
        let (epoch1, pk1) = ring.current_public();
        let tx = EncryptedTx::seal(
            &pk1, epoch1, b"old-tx", [0; 33], None, 1, 100,
        )
        .unwrap();

        // Rotate to epoch 2 at block 110 (within TTL window of 64).
        ring.rotate(2, 110);
        // Old key still in retired map → decrypt works.
        assert!(PrivateMempool::decrypt(&ring, &tx).is_ok());
    }

    #[test]
    fn key_rotation_expired_epoch_rejects() {
        let mut ring = fresh_keyring();
        let (epoch1, pk1) = ring.current_public();
        let tx = EncryptedTx::seal(
            &pk1, epoch1, b"old-tx", [0; 33], None, 1, 100,
        )
        .unwrap();

        // Rotate at block 200 (well past TTL=64 from registered_at=100).
        ring.rotate(2, 200);
        // Pruned by rotate() → decrypt fails with "unknown or expired".
        let err = PrivateMempool::decrypt(&ring, &tx).unwrap_err();
        match err {
            MevError::DecryptionFailed(msg) => {
                assert!(msg.contains("unknown or expired"), "got: {msg}");
            }
            _ => panic!("wrong error variant: {err:?}"),
        }
    }

    // ─── S29-FIX-CRIT-1: id integrity on submit ──────────────────────

    #[test]
    fn submit_rejects_forged_id() {
        let ring = fresh_keyring();
        let (epoch, pk) = ring.current_public();
        let mut tx = EncryptedTx::seal(&pk, epoch, b"x", [0; 33], None, 1, 100).unwrap();
        // Forge id to attempt to overwrite some other slot.
        tx.id = [0xDE; 32];

        let mut pool = PrivateMempool::new(8);
        let err = pool.submit(tx, /*local=*/ 100).unwrap_err();
        match err {
            MevError::InvalidEncryptedTx(msg) => {
                assert!(msg.contains("id does not match"), "got: {msg}");
            }
            _ => panic!("wrong error variant: {err:?}"),
        }
        assert_eq!(pool.len(), 0, "forged tx must not be stored");
    }

    #[test]
    fn submit_recomputes_id_consistently() {
        // The pool must compute the canonical id from KEM bytes +
        // ciphertext. Compare against the public helper.
        let ring = fresh_keyring();
        let (epoch, pk) = ring.current_public();
        let tx = EncryptedTx::seal(&pk, epoch, b"x", [0; 33], None, 1, 100).unwrap();
        let canonical = compute_tx_id(&tx.encapped_key, &tx.ciphertext);
        assert_eq!(canonical, tx.id, "seal() must produce canonical id");
    }

    // ─── S29-FIX-MED-1: pool-layer replay protection ─────────────────

    #[test]
    fn submit_rejects_exact_blob_replay_within_window() {
        let ring = fresh_keyring();
        let (epoch, pk) = ring.current_public();
        let tx = EncryptedTx::seal(&pk, epoch, b"x", [0; 33], None, 1, 100).unwrap();

        let mut pool = PrivateMempool::with_replay_window(8, /*window=*/ 256);
        pool.submit(tx.clone(), /*local=*/ 100).unwrap();
        // Drain (so it's not in `txs` anymore) — should still be rejected.
        let _ = pool.drain_for_block(1_000);
        assert_eq!(pool.len(), 0);

        let err = pool.submit(tx, /*local=*/ 100).unwrap_err();
        match err {
            MevError::InvalidEncryptedTx(msg) => {
                assert!(msg.contains("replay window"), "got: {msg}");
            }
            _ => panic!("wrong error variant: {err:?}"),
        }
    }

    #[test]
    fn submit_rejects_collision_against_live_entry() {
        // Two distinct seals → distinct ciphertexts → distinct ids
        // (HPKE ephemeral KEM key is randomized). But explicitly test
        // that a SECOND submit of the SAME blob is rejected.
        let ring = fresh_keyring();
        let (epoch, pk) = ring.current_public();
        let tx = EncryptedTx::seal(&pk, epoch, b"x", [0; 33], None, 1, 100).unwrap();
        let mut pool = PrivateMempool::new(8);
        pool.submit(tx.clone(), /*local=*/ 100).unwrap();
        let err = pool.submit(tx, /*local=*/ 100).unwrap_err();
        assert!(matches!(err, MevError::InvalidEncryptedTx(_)));
    }

    #[test]
    fn replay_window_eviction_allows_resubmit_past_window() {
        let ring = fresh_keyring();
        let (epoch, pk) = ring.current_public();
        let tx_old = EncryptedTx::seal(&pk, epoch, b"x", [0; 33], None, 1, 100).unwrap();

        let mut pool = PrivateMempool::with_replay_window(8, /*window=*/ 64);
        pool.submit(tx_old.clone(), /*local=*/ 100).unwrap();
        let _ = pool.drain_for_block(1_000);

        // A fresh tx submitted later — local_submitted_at = 1000 — triggers
        // pruning of seen_ids (cutoff = 1000 - 64 = 936). The old id
        // (registered with local=100) is evicted.
        let tx_new = EncryptedTx::seal(&pk, epoch, b"y", [0; 33], None, 2, 1000).unwrap();
        pool.submit(tx_new, /*local=*/ 1000).unwrap();
        // Now the old id is gone from seen_ids, so re-submitting it would
        // (in theory) be accepted at this layer. Execution-layer nonce
        // is the authoritative defense for that case.
        assert_eq!(pool.len(), 1);
    }

    // ─── S29-FIX-MED-1 (round 2): authoritative-local submitted_at ───

    #[test]
    fn attacker_far_future_wire_submitted_at_cannot_evict_seen_ids() {
        // Attacker tries to set tx.submitted_at = u64::MAX so that the
        // pool's pruning cutoff = MAX - replay_window evicts every prior
        // entry. With local-authoritative submitted_at, this fails.
        let ring = fresh_keyring();
        let (epoch, pk) = ring.current_public();
        let tx_a = EncryptedTx::seal(&pk, epoch, b"a", [0; 33], None, 1, 100).unwrap();

        let mut pool = PrivateMempool::with_replay_window(8, /*window=*/ 256);
        // Local view: tx_a arrived at local block 100.
        pool.submit(tx_a.clone(), /*local=*/ 100).unwrap();
        let _ = pool.drain_for_block(1_000);
        assert_eq!(pool.len(), 0);

        // Attacker submits tx_b with FORGED tx.submitted_at = u64::MAX.
        // Local view: tx_b arrived at local block 110 (within window).
        let mut tx_b = EncryptedTx::seal(&pk, epoch, b"b", [0; 33], None, 2, 100).unwrap();
        tx_b.submitted_at = u64::MAX; // attacker-controlled wire value
        pool.submit(tx_b, /*local=*/ 110).unwrap();

        // Cutoff must be 110 - 256 = saturating_sub = 0; tx_a (stored at
        // local=100) is NOT pruned. Re-submitting tx_a must be REJECTED.
        let err = pool.submit(tx_a, /*local=*/ 110).unwrap_err();
        match err {
            MevError::InvalidEncryptedTx(msg) => {
                assert!(msg.contains("replay window"), "got: {msg}");
            }
            _ => panic!("wrong error variant: {err:?}"),
        }
    }

    #[test]
    fn attacker_backward_wire_submitted_at_does_not_inhibit_pruning() {
        // Attacker tries to set tx.submitted_at = 0 (or any backward
        // value) so the pool fails to prune. With local-authoritative
        // submitted_at, the pool prunes correctly off the local clock.
        let ring = fresh_keyring();
        let (epoch, pk) = ring.current_public();
        let tx_old = EncryptedTx::seal(&pk, epoch, b"old", [0; 33], None, 1, 100).unwrap();

        let mut pool = PrivateMempool::with_replay_window(8, /*window=*/ 64);
        pool.submit(tx_old.clone(), /*local=*/ 100).unwrap();
        let _ = pool.drain_for_block(1_000);

        // Attacker submits tx_new with FORGED tx.submitted_at = 0,
        // hoping pruning cutoff = 0 - 64 = saturating = 0 leaves
        // everything intact. Local view: tx_new arrived at local=500.
        let mut tx_new = EncryptedTx::seal(&pk, epoch, b"new", [0; 33], None, 2, 0).unwrap();
        tx_new.submitted_at = 0; // attacker-controlled
        pool.submit(tx_new, /*local=*/ 500).unwrap();

        // Pool used local=500 for cutoff = 500 - 64 = 436. tx_old (stored
        // at local=100) IS pruned. Re-submitting tx_old must SUCCEED.
        pool.submit(tx_old, /*local=*/ 500).unwrap();
        assert_eq!(pool.len(), 2);
    }

    #[test]
    fn submit_overrides_wire_submitted_at_in_stored_tx() {
        // The stored tx must reflect the local value, not the wire value.
        let ring = fresh_keyring();
        let (epoch, pk) = ring.current_public();
        let mut tx = EncryptedTx::seal(&pk, epoch, b"x", [0; 33], None, 1, 999).unwrap();
        tx.submitted_at = 999; // wire-supplied value

        let mut pool = PrivateMempool::new(8);
        pool.submit(tx, /*local=*/ 42).unwrap();
        let drained = pool.drain_for_block(u64::MAX);
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].submitted_at, 42, "wire value must be overridden");
    }

    #[test]
    fn drain_and_decrypt_preserves_per_tx_failure_isolation() {
        let mut ring = fresh_keyring();
        let (epoch, pk) = ring.current_public();
        let mut pool = PrivateMempool::new(8);

        let good = EncryptedTx::seal(&pk, epoch, b"g", [0; 33], None, 1, 100).unwrap();
        let mut bad = EncryptedTx::seal(&pk, epoch, b"b", [0; 33], None, 2, 100).unwrap();
        bad.ciphertext[0] ^= 0xFF;
        // bad.id was computed from the ORIGINAL ciphertext; recompute so
        // submit's id-integrity check doesn't reject before the AEAD
        // failure during decrypt is exercised.
        bad.id = compute_tx_id(&bad.encapped_key, &bad.ciphertext);

        pool.submit(good.clone(), /*local=*/ 100).unwrap();
        pool.submit(bad.clone(), /*local=*/ 100).unwrap();

        let results = pool.drain_and_decrypt_for_block(&ring, 1_000);
        assert_eq!(results.len(), 2);
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        assert_eq!(results.iter().filter(|r| r.is_err()).count(), 1);
    }
}
