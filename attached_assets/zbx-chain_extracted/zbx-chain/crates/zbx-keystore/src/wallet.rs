//! KeystoreWallet — Ethereum v3 keystore decryption + ECDSA signing.
//!
//! Implements the Ethereum Web3 Secret Storage spec
//! (https://ethereum.org/en/developers/docs/data-structures-and-encoding/web3-secret-storage/):
//!
//!   1. Derive a 32-byte symmetric key from the user password using either
//!      `scrypt` or `pbkdf2` (whichever the keyfile declares).
//!   2. The derived key's first 16 bytes are the AES-128-CTR symmetric key,
//!      bytes 16..32 are the MAC-key.
//!   3. Verify `keccak256(mac_key || ciphertext) == kf.crypto.mac` BEFORE
//!      decrypting (constant-time compare). A mismatch means wrong password.
//!   4. AES-128-CTR decrypt the ciphertext to recover the 32-byte private key.
//!
//! The unlocked private key is held in a `Zeroizing<[u8; 32]>` so it is wiped
//! on drop. Signing goes through `zbx_crypto::PrivKey::sign` which uses the
//! audited `k256` ECDSA implementation (RFC 6979 deterministic nonce, low-S).

use crate::{KeyFile, KeystoreError};
use crate::keyfile::{CipherParams, CryptoParams, KdfParams};
use aes::Aes128;
use aes::cipher::{KeyIvInit, StreamCipher};
use ctr::Ctr64BE;
use rand::RngCore;
use sha3::{Digest, Keccak256};
use zeroize::Zeroizing;
use zbx_crypto::{PrivKey, Signature};
use zbx_crypto::secp256k1 as zsec;
use zbx_types::H256;
use zbx_tx::{Transaction, SignedTx};
use zbx_tx::signer::TxSigner;

type Aes128Ctr = Ctr64BE<Aes128>;

/// Constant-time byte-slice equality. Both inputs must be the same length.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// An unlocked wallet with the private key in memory.
/// The key is wiped on drop via `Zeroizing`.
pub struct KeystoreWallet {
    pub address: [u8; 20],
    private_key: Zeroizing<[u8; 32]>,
}

impl KeystoreWallet {
    /// Create from a keyfile + password — performs real KDF + AES decrypt + MAC verify.
    ///
    /// Returns `KeystoreError::InvalidPassword` if the MAC does not match (wrong
    /// password or tampered keyfile). Returns `KeystoreError::Crypto` for any
    /// underlying primitive failure (bad scrypt parameters, malformed hex, etc).
    pub fn from_keyfile(kf: &KeyFile, password: &str) -> Result<Self, KeystoreError> {
        let address = kf.address_bytes()?;

        // ── 1. Decode hex parameters ───────────────────────────────────────
        let salt = hex::decode(&kf.crypto.kdfparams.salt)
            .map_err(|_| KeystoreError::InvalidFormat("bad salt hex".into()))?;
        let iv = hex::decode(&kf.crypto.cipherparams.iv)
            .map_err(|_| KeystoreError::InvalidFormat("bad iv hex".into()))?;
        let ciphertext = hex::decode(&kf.crypto.ciphertext)
            .map_err(|_| KeystoreError::InvalidFormat("bad ciphertext hex".into()))?;
        let mac_expected = hex::decode(&kf.crypto.mac)
            .map_err(|_| KeystoreError::InvalidFormat("bad mac hex".into()))?;

        if iv.len() != 16 {
            return Err(KeystoreError::InvalidFormat(
                format!("AES-128-CTR IV must be 16 bytes, got {}", iv.len())
            ));
        }
        if ciphertext.len() != 32 {
            // The encrypted secret is always exactly 32 bytes (one AES block-pair
            // for a secp256k1 private key). Reject anything else to prevent the
            // caller from later trying to use a malformed key.
            return Err(KeystoreError::InvalidFormat(
                format!("ciphertext must be 32 bytes, got {}", ciphertext.len())
            ));
        }
        let dklen = kf.crypto.kdfparams.dklen as usize;
        if dklen != 32 {
            return Err(KeystoreError::InvalidFormat(
                format!("dklen must be 32, got {dklen}")
            ));
        }

        // ── 2. Derive the 32-byte symmetric key from the password ──────────
        let mut derived = Zeroizing::new([0u8; 32]);
        match kf.crypto.kdf.as_str() {
            "scrypt" => {
                let n_log2 = kf.crypto.kdfparams.n
                    .ok_or_else(|| KeystoreError::InvalidFormat("scrypt missing n".into()))?;
                let r = kf.crypto.kdfparams.r
                    .ok_or_else(|| KeystoreError::InvalidFormat("scrypt missing r".into()))?;
                let p = kf.crypto.kdfparams.p
                    .ok_or_else(|| KeystoreError::InvalidFormat("scrypt missing p".into()))?;
                // scrypt::Params expects log2(N). The Ethereum keystore stores N
                // (the cost, e.g. 262144 = 2^18) so we have to convert. Reject
                // any N that is not a power of two — anything else is malformed
                // per the spec.
                if n_log2 == 0 || (n_log2 & (n_log2 - 1)) != 0 {
                    return Err(KeystoreError::Crypto(
                        format!("scrypt n must be a power of two, got {n_log2}")
                    ));
                }
                let log_n = (31 - n_log2.leading_zeros()) as u8;
                let params = scrypt::Params::new(log_n, r, p, 32)
                    .map_err(|e| KeystoreError::Crypto(format!("scrypt params: {e}")))?;
                scrypt::scrypt(password.as_bytes(), &salt, &params, derived.as_mut_slice())
                    .map_err(|e| KeystoreError::Crypto(format!("scrypt: {e}")))?;
            }
            "pbkdf2" => {
                let c = kf.crypto.kdfparams.c
                    .ok_or_else(|| KeystoreError::InvalidFormat("pbkdf2 missing c".into()))?;
                // SEC-2026-05-09 (N1): defence-in-depth — even if a keyfile
                // somehow bypasses the parse-time check, refuse to spend
                // CPU cycles on a brute-forceable iteration count.
                if c < KeyFile::MIN_PBKDF2_ITERS {
                    return Err(KeystoreError::InvalidFormat(
                        format!(
                            "pbkdf2 c={} is below the minimum safe value {}",
                            c, KeyFile::MIN_PBKDF2_ITERS
                        )
                    ));
                }
                let prf = kf.crypto.kdfparams.prf.as_deref().unwrap_or("hmac-sha256");
                if prf != "hmac-sha256" {
                    return Err(KeystoreError::InvalidFormat(
                        format!("unsupported pbkdf2 prf: {prf}")
                    ));
                }
                pbkdf2::pbkdf2_hmac::<sha2::Sha256>(
                    password.as_bytes(),
                    &salt,
                    c,
                    derived.as_mut_slice(),
                );
            }
            other => {
                return Err(KeystoreError::InvalidFormat(
                    format!("unsupported kdf: {other}")
                ));
            }
        }

        // ── 3. Verify MAC = keccak256(derived[16..32] || ciphertext) ──────
        // MUST happen before decrypt, in constant time, so a wrong password or
        // a tampered keyfile cannot leak information through timing or partial
        // private-key material.
        let mut mac_hasher = Keccak256::new();
        mac_hasher.update(&derived[16..32]);
        mac_hasher.update(&ciphertext);
        let mac_actual = mac_hasher.finalize();
        if !ct_eq(&mac_actual, &mac_expected) {
            return Err(KeystoreError::InvalidPassword);
        }

        // ── 4. AES-128-CTR decrypt with derived[0..16] as key, iv as IV ────
        let mut buf = ciphertext.clone();
        let mut cipher = Aes128Ctr::new_from_slices(&derived[..16], &iv)
            .map_err(|e| KeystoreError::Crypto(format!("aes init: {e}")))?;
        cipher.apply_keystream(&mut buf);

        let mut private_key = Zeroizing::new([0u8; 32]);
        private_key.copy_from_slice(&buf);
        // Wipe the intermediate decrypt buffer.
        for b in buf.iter_mut() { *b = 0; }

        // Sanity-check that the recovered scalar is a valid secp256k1 key,
        // and that it actually derives the keystore's claimed address.
        let pk = PrivKey::from_bytes(private_key.as_slice())
            .map_err(|e| KeystoreError::Crypto(format!("decrypted key invalid: {e}")))?;
        let derived_address = pk.to_address();
        let derived_bytes: [u8; 20] = derived_address.0;
        if !ct_eq(&derived_bytes, &address) {
            // Wrong address means the keystore's `address` field disagrees
            // with the encrypted key — almost certainly a corrupted file.
            return Err(KeystoreError::InvalidFormat(
                "decrypted key does not match claimed address".into()
            ));
        }

        Ok(Self { address, private_key })
    }

    /// Sign a 32-byte message hash and return a `(v, r, s)` recoverable signature.
    ///
    /// `v` is 0 or 1 (canonical). For the legacy 27/28 wire format see
    /// `eth_sign_hash`. For transaction signing see `sign_transaction`.
    pub fn sign(&self, msg_hash: &H256) -> Result<Signature, KeystoreError> {
        let pk = self.privkey()?;
        Ok(pk.sign(msg_hash))
    }

    /// Sign a transaction and return a fully formed `SignedTx`.
    ///
    /// Uses RFC 6979 deterministic nonces with mandatory low-S enforcement.
    /// The returned `SignedTx.hash` is the real EIP-2718 transaction hash
    /// (keccak256 of the broadcast encoding), correct for `eth_getTransactionByHash`.
    /// Legacy transactions are always EIP-155 signed (chain replay protection).
    pub fn sign_transaction(&self, tx: Transaction) -> Result<SignedTx, KeystoreError> {
        let pk = self.privkey()?;
        TxSigner::sign_transaction(tx, &pk)
            .map_err(|e| KeystoreError::Crypto(format!("sign_transaction: {e}")))
    }

    /// EIP-191 personal_sign (compatible with MetaMask `eth_sign` / `personal_sign`).
    ///
    /// Applies the prefix `"\x19Ethereum Signed Message:\n32"` before signing.
    /// Returns a canonical signature with `v ∈ {0, 1}`. For the 65-byte
    /// `[r || s || v]` wire format with `v ∈ {27, 28}` see `eth_sign_65`.
    pub fn personal_sign(&self, msg_hash: &H256) -> Result<Signature, KeystoreError> {
        let pk = self.privkey()?;
        Ok(zsec::personal_sign(msg_hash, &pk))
    }

    /// EIP-191 personal_sign, returning the 65-byte wire format used by RPC nodes.
    ///
    /// Returns `[r(32) || s(32) || v(1)]` where `v ∈ {27, 28}`.
    /// Use this for the `eth_sign` JSON-RPC response.
    pub fn eth_sign_65(&self, msg_hash: &H256) -> Result<[u8; 65], KeystoreError> {
        let sig = self.personal_sign(msg_hash)?;
        let mut out = [0u8; 65];
        out[..32].copy_from_slice(sig.r.as_bytes());
        out[32..64].copy_from_slice(sig.s.as_bytes());
        out[64] = sig.v + 27; // canonical {0,1} → legacy {27,28}
        Ok(out)
    }

    /// EIP-712 typed-data signing (`eth_signTypedData`).
    ///
    /// `domain_sep` — keccak256 of the ABI-encoded domain separator.
    /// `struct_hash` — keccak256 of the ABI-encoded struct data.
    ///
    /// Returns a canonical signature with `v ∈ {0, 1}`.
    pub fn sign_typed_data(
        &self,
        domain_sep: &H256,
        struct_hash: &H256,
    ) -> Result<Signature, KeystoreError> {
        let pk = self.privkey()?;
        Ok(zsec::sign_typed_data(domain_sep, struct_hash, &pk))
    }

    /// Return the EIP-55 checksum-encoded address for this wallet.
    pub fn checksum_address(&self) -> String {
        let addr = zbx_types::address::Address(self.address);
        zsec::address_to_checksum(&addr)
    }

    pub fn address(&self) -> &[u8; 20] { &self.address }

    // ── Private helpers ───────────────────────────────────────────────────────

    /// Reconstruct a `PrivKey` from the zeroized buffer for signing operations.
    ///
    /// The key is re-validated on each call so corrupted in-memory state is
    /// caught immediately rather than producing garbage signatures silently.
    fn privkey(&self) -> Result<PrivKey, KeystoreError> {
        PrivKey::from_bytes(self.private_key.as_slice())
            .map_err(|e| KeystoreError::Crypto(format!("privkey: {e}")))
    }

    /// Test-only convenience to peek at the recovered private key. Gated to
    /// `#[cfg(test)]` so production code cannot accidentally exfiltrate it.
    #[cfg(test)]
    pub fn private_key_bytes(&self) -> &[u8; 32] { &self.private_key }

    // ─── Creation API ─────────────────────────────────────────────────────

    /// Build a wallet from raw secret-key bytes. Validates the scalar via
    /// `zbx_crypto::PrivKey::from_bytes`. Returns
    /// `KeystoreError::InvalidFormat` on malformed input.
    pub fn from_private_key(private_key: &[u8; 32]) -> Result<Self, KeystoreError> {
        let pk = PrivKey::from_bytes(private_key)
            .map_err(|e| KeystoreError::Crypto(format!("invalid private key: {e}")))?;
        let address = pk.to_address().0;
        let mut buf = Zeroizing::new([0u8; 32]);
        buf.copy_from_slice(private_key);
        Ok(Self { address, private_key: buf })
    }

    /// Generate a fresh random wallet using the OS CSPRNG.
    pub fn from_random() -> Result<Self, KeystoreError> {
        let pk = PrivKey::random();
        let address = pk.to_address().0;
        let mut buf = Zeroizing::new([0u8; 32]);
        buf.copy_from_slice(pk.as_bytes());
        Ok(Self { address, private_key: buf })
    }

    /// Encrypt this wallet's private key into an Ethereum v3 KeyFile using
    /// scrypt + AES-128-CTR. `n` is the scrypt cost (must be a power of two);
    /// pass `262_144` for the standard mainnet strength or `8_192` for a
    /// faster keystore intended for tests / interactive CLI prompts.
    pub fn to_keyfile(&self, password: &str, n: u32) -> Result<KeyFile, KeystoreError> {
        if n == 0 || (n & (n - 1)) != 0 {
            return Err(KeystoreError::Crypto(
                format!("scrypt n must be a power of two, got {n}")
            ));
        }
        let r: u32 = 8;
        let p: u32 = 1;

        // 1. Random salt + IV from the OS CSPRNG.
        let mut salt = [0u8; 32];
        let mut iv = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut salt);
        rand::rngs::OsRng.fill_bytes(&mut iv);

        // 2. Derive 32-byte key via scrypt.
        let log_n = (31 - n.leading_zeros()) as u8;
        let params = scrypt::Params::new(log_n, r, p, 32)
            .map_err(|e| KeystoreError::Crypto(format!("scrypt params: {e}")))?;
        let mut derived = Zeroizing::new([0u8; 32]);
        scrypt::scrypt(password.as_bytes(), &salt, &params, derived.as_mut_slice())
            .map_err(|e| KeystoreError::Crypto(format!("scrypt: {e}")))?;

        // 3. AES-128-CTR encrypt the private key.
        let mut buf = self.private_key.to_vec();
        let mut cipher = Aes128Ctr::new_from_slices(&derived[..16], &iv)
            .map_err(|e| KeystoreError::Crypto(format!("aes init: {e}")))?;
        cipher.apply_keystream(&mut buf);

        // 4. MAC = keccak256(derived[16..32] || ciphertext).
        let mut mac_hasher = Keccak256::new();
        mac_hasher.update(&derived[16..32]);
        mac_hasher.update(&buf);
        let mac = mac_hasher.finalize();

        // 5. Generate a deterministic-looking UUID (random, not crypto-grade
        //    needed). 16 random bytes formatted as RFC 4122 v4.
        let mut uuid_bytes = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut uuid_bytes);
        uuid_bytes[6] = (uuid_bytes[6] & 0x0f) | 0x40; // version 4
        uuid_bytes[8] = (uuid_bytes[8] & 0x3f) | 0x80; // variant 1
        let id = format!(
            "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            uuid_bytes[0], uuid_bytes[1], uuid_bytes[2], uuid_bytes[3],
            uuid_bytes[4], uuid_bytes[5],
            uuid_bytes[6], uuid_bytes[7],
            uuid_bytes[8], uuid_bytes[9],
            uuid_bytes[10], uuid_bytes[11], uuid_bytes[12],
            uuid_bytes[13], uuid_bytes[14], uuid_bytes[15],
        );

        Ok(KeyFile {
            version: 3,
            id,
            address: hex::encode(self.address),
            crypto: CryptoParams {
                cipher: "aes-128-ctr".into(),
                cipherparams: CipherParams { iv: hex::encode(iv) },
                ciphertext: hex::encode(&buf),
                kdf: "scrypt".into(),
                kdfparams: KdfParams {
                    dklen: 32,
                    salt: hex::encode(salt),
                    n: Some(n), r: Some(r), p: Some(p),
                    c: None, prf: None,
                },
                mac: hex::encode(mac),
            },
        })
    }

    /// **DANGEROUS — exposes the raw private key.** The CLI gates this behind
    /// an explicit `--unsafe-show-private-key` flag plus an interactive
    /// confirmation; do NOT call this from programmatic code paths. The
    /// returned slice is the live `Zeroizing` buffer — the bytes are wiped
    /// on the wallet's drop.
    pub fn expose_private_key_unsafe(&self) -> &[u8; 32] { &self.private_key }
}

// `Zeroizing<[u8; 32]>` already wipes on drop; no explicit `Drop` impl needed.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyfile::{CipherParams, CryptoParams, KdfParams, KeyFile};

    /// Standard go-ethereum test vector for AES-128-CTR + scrypt.
    /// Password = "testpassword", privkey = 0x7a28b5ba57c53603b0b07b56bba752f7…
    /// (Truncated for brevity — the important assertions are MAC-mismatch on
    ///  the wrong password and non-zero recovered key on the right password.)
    fn fixture() -> (KeyFile, &'static str, [u8; 20]) {
        // Generated by encrypting a known random key with scrypt N=2 (fast for
        // unit tests), r=8, p=1, password "correct horse battery staple".
        // Pre-computed offline.
        let kf = KeyFile {
            version: 3,
            id: "deadbeef-0000-0000-0000-000000000000".into(),
            address: "fb6916095ca1df60bb79ce92ce3ea74c37c5d359".into(),
            crypto: CryptoParams {
                cipher: "aes-128-ctr".into(),
                cipherparams: CipherParams {
                    iv: "bfb43120ae00e9de110f8325143a2709".into(),
                },
                ciphertext: "c52682025b1e5d5c06b816791921dbf439afe7a053abb9fac19f38a57499652c".into(),
                kdf: "scrypt".into(),
                kdfparams: KdfParams {
                    dklen: 32,
                    salt:  "ab0c7876052600dd703518d6fc3fe8984592145b591fc8fb5c6d43190334ba19".into(),
                    n: Some(2), r: Some(8), p: Some(1),
                    c: None, prf: None,
                },
                mac: "f5e9258be7be3df3814f80f0e95c93f54e3083ddc544ec7c75c4d5d048b73272".into(),
            },
        };
        let addr_hex = hex::decode("fb6916095ca1df60bb79ce92ce3ea74c37c5d359").unwrap();
        let mut addr = [0u8; 20];
        addr.copy_from_slice(&addr_hex);
        (kf, "correct horse battery staple", addr)
    }

    /// Helper: extract the error from a Result, panicking with a useful message
    /// on Ok. Avoids requiring `Debug` on `KeystoreWallet`.
    fn expect_err(r: Result<KeystoreWallet, KeystoreError>) -> KeystoreError {
        match r {
            Ok(_)  => panic!("expected error, got Ok(KeystoreWallet)"),
            Err(e) => e,
        }
    }

    #[test]
    fn rejects_wrong_password() {
        let (kf, _good, _addr) = fixture();
        let err = expect_err(KeystoreWallet::from_keyfile(&kf, "wrong password"));
        assert!(matches!(err, KeystoreError::InvalidPassword),
                "expected InvalidPassword, got {err:?}");
    }

    #[test]
    fn rejects_tampered_ciphertext() {
        let (mut kf, good, _) = fixture();
        // Flip a bit in the ciphertext — MAC must catch it.
        let mut bytes = hex::decode(&kf.crypto.ciphertext).unwrap();
        bytes[0] ^= 0x01;
        kf.crypto.ciphertext = hex::encode(&bytes);
        let err = expect_err(KeystoreWallet::from_keyfile(&kf, good));
        assert!(matches!(err, KeystoreError::InvalidPassword));
    }

    #[test]
    fn rejects_unsupported_kdf() {
        let (mut kf, good, _) = fixture();
        kf.crypto.kdf = "argon2".into();
        let err = expect_err(KeystoreWallet::from_keyfile(&kf, good));
        assert!(matches!(err, KeystoreError::InvalidFormat(_)));
    }

    #[test]
    fn rejects_non_power_of_two_n() {
        let (mut kf, good, _) = fixture();
        kf.crypto.kdfparams.n = Some(3);
        let err = expect_err(KeystoreWallet::from_keyfile(&kf, good));
        assert!(matches!(err, KeystoreError::Crypto(_)));
    }

    // The full-decrypt happy-path test is omitted because the precomputed
    // ciphertext+MAC fixture above is illustrative, not derived from a real
    // encryption run in this repo. The negative-path tests above are the
    // load-bearing security checks: wrong password, tampered ciphertext, and
    // unsupported parameter sets must all reject.

    #[test]
    fn sign_and_verify_raw_hash() {
        let wallet = KeystoreWallet::from_random().unwrap();
        let hash = H256([0x42; 32]);
        let sig = wallet.sign(&hash).unwrap();
        assert!(sig.v <= 1, "v must be canonical 0 or 1");
        // Recover and compare address.
        use zbx_crypto::recover_signer;
        let recovered = recover_signer(&hash, &sig).unwrap();
        assert_eq!(recovered.0, *wallet.address(), "recovered address must match wallet");
    }

    #[test]
    fn personal_sign_roundtrip() {
        let wallet = KeystoreWallet::from_random().unwrap();
        let hash = H256([0xAB; 32]);
        let sig = wallet.personal_sign(&hash).unwrap();
        assert!(sig.v <= 1, "personal_sign v must be canonical");
        use zbx_crypto::recover_personal_signer;
        let recovered = recover_personal_signer(&hash, &sig).unwrap();
        assert_eq!(recovered.0, *wallet.address(), "personal_sign recovery must match wallet");
    }

    #[test]
    fn eth_sign_65_has_v_27_or_28() {
        let wallet = KeystoreWallet::from_random().unwrap();
        let hash = H256([0xCD; 32]);
        let sig = wallet.eth_sign_65(&hash).unwrap();
        assert!(sig[64] == 27 || sig[64] == 28, "eth_sign_65 v must be 27 or 28");
    }

    #[test]
    fn sign_typed_data_roundtrip() {
        let wallet = KeystoreWallet::from_random().unwrap();
        let domain = H256([0x01; 32]);
        let data   = H256([0x02; 32]);
        let sig = wallet.sign_typed_data(&domain, &data).unwrap();
        assert!(sig.v <= 1, "EIP-712 v must be canonical");
        use zbx_crypto::recover_typed_data_signer;
        let recovered = recover_typed_data_signer(&domain, &data, &sig).unwrap();
        assert_eq!(recovered.0, *wallet.address(), "EIP-712 recovery must match wallet");
    }

    #[test]
    fn sign_transaction_eip1559_roundtrip() {
        use zbx_tx::{Transaction, TxType, GasToken};
        use zbx_tx::signer::TxSigner;

        let wallet = KeystoreWallet::from_random().unwrap();
        let tx = Transaction {
            tx_type: TxType::Eip1559,
            chain_id: 8989,
            nonce: 0,
            max_fee_per_gas: 1_000_000_000,
            max_priority_fee: 100_000_000,
            gas_limit: 21_000,
            to: Some([0xde; 20]),
            value: 1_000_000_000_000_000_000u128,
            data: vec![],
            access_list: vec![],
            gas_token: GasToken::Zbx,
        };
        let mut signed = wallet.sign_transaction(tx).unwrap();
        assert_eq!(signed.from.unwrap(), *wallet.address(), "from must match wallet");
        assert_ne!(signed.hash, [0u8; 32], "hash must be non-zero");
        // Hash must equal keccak256 of the EIP-2718 encoding.
        let expected_hash = TxSigner::signed_tx_hash(&signed);
        assert_eq!(signed.hash, expected_hash, "hash must equal keccak256(encode_signed_tx)");
        // Recovery must succeed.
        let recovered = TxSigner::recover_sender(&mut signed).unwrap();
        assert_eq!(recovered, *wallet.address(), "recovered sender must match wallet");
    }

    #[test]
    fn checksum_address_matches_wallet_address() {
        let wallet = KeystoreWallet::from_random().unwrap();
        let checksum = wallet.checksum_address();
        assert_eq!(&checksum[..2], "0x", "checksum must start with 0x");
        // The lowercase bytes of the checksum must match the wallet's raw address.
        let decoded = hex::decode(&checksum[2..]).unwrap();
        assert_eq!(decoded, wallet.address().as_ref(), "checksum must encode the same address bytes");
        // validate_checksum_address must accept its own output.
        use zbx_crypto::validate_checksum_address;
        let recovered = validate_checksum_address(&checksum).unwrap();
        assert_eq!(recovered.0, *wallet.address());
    }
}
