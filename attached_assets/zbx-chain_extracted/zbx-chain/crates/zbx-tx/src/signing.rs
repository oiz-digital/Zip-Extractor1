//! High-level transaction signing convenience API.
//!
//! `signing.rs` wraps the low-level `TxSigner` in `signer.rs` and provides:
//!
//! * `SigningContext` — immutable per-chain parameters (chain_id, gas_token
//!   defaults) used to construct signing envelopes without boilerplate.
//! * `sign_transfer` / `sign_contract_call` / `sign_deploy` helpers that
//!   produce a ready-to-broadcast `SignedTx` in a single call.
//! * `batch_sign` — sign multiple transactions with monotonically-increasing
//!   nonces so wallets can queue without re-querying the mempool.
//!
//! ## Chain IDs
//!
//! | Network  | Chain ID |
//! |----------|----------|
//! | Mainnet  | 8989     |
//! | Testnet  | 8990     |
//! | Devnet   | 8990     |  ← same on-chain id as testnet (EIP-155 alone is insufficient)
//!
//! These match the genesis `chainId` and are enforced in EIP-155 replay
//! protection (`Transaction::chain_id` field) for **on-chain** transactions.
//!
//! ## Cross-Network Replay Prevention (P0-T07)
//!
//! Devnet and testnet intentionally share chain_id 8990, which means
//! **on-chain EIP-155 replay protection does NOT separate them**.  A signed
//! transaction broadcast on devnet is byte-identical to one valid on testnet.
//!
//! For **on-chain transactions** this is acceptable during early testnet
//! because both networks use throw-away keys and disposable state.  The
//! operational rule is: never reuse a private key across devnet and testnet.
//!
//! For **off-chain personal-sign messages** (paymaster validation, session
//! keys, bridge relay proofs) the risk is higher: a message signed for devnet
//! can be replayed on testnet if the verifying contract does not bind the
//! network.  To prevent this, every off-chain signing flow MUST prefix its
//! message with the `NetworkSigningDomain` byte-string before hashing:
//!
//! ```text
//! digest = keccak256(network_domain_tag || application_message)
//! sig    = personal_sign(digest, privkey)
//! ```
//!
//! The `NetworkSigningDomain` constants below provide the canonical tags.
//! See `SigningContext::network_domain()` for the per-context accessor.
//!
//! Once devnet graduates to its own chain_id the domain tags can be removed
//! and standard EIP-155 protection will suffice.  Track in issue P0-T07.

use crate::{
    error::TxError,
    signer::TxSigner,
    types::{AccessListEntry, GasToken, SignedTx, Transaction, TxType},
};
use zbx_crypto::secp256k1::PrivKey;
use zbx_types::address::Address;

// ── Chain IDs ─────────────────────────────────────────────────────────────────

pub const CHAIN_ID_MAINNET: u64 = 8989;
pub const CHAIN_ID_TESTNET: u64 = 8990;
/// Devnet deliberately shares the testnet chain_id (8990) during the early
/// network phase.  Once devnet graduates to a permanent ID this constant
/// will diverge.  See the module-level doc for the cross-network replay
/// mitigation required while the two share a chain_id.
pub const CHAIN_ID_DEVNET: u64 = 8990;

// ── Off-chain signing domain tags (P0-T07) ────────────────────────────────────
//
// Because devnet and testnet share chain_id 8990, EIP-155 replay protection
// alone cannot separate personal-sign messages across the two networks.
// Every off-chain signing flow (paymaster, session key, bridge relay proof,
// typed-data RPC) MUST prepend the appropriate domain tag to its message
// before hashing so that a signature produced on one network is invalid on
// the other.
//
// Usage:
//     let domain = ctx.network_domain();
//     let mut pre = Vec::with_capacity(domain.len() + msg.len());
//     pre.extend_from_slice(domain);
//     pre.extend_from_slice(msg);
//     let digest = zbx_crypto::keccak::keccak256(&pre);
//     let sig    = zbx_crypto::secp256k1::personal_sign(&digest, key);
//
// Verifiers (e.g. ZbxPaymaster.sol / EntryPoint) must apply the SAME prefix
// before recovering the signer — failing to do so makes them vulnerable to
// cross-network replay.

/// Off-chain domain tag for Zebvix **mainnet** (chain_id 8989).
/// This is distinct from the testnet/devnet tags so mainnet messages cannot
/// be replayed on test networks and vice-versa.
pub const SIGNING_DOMAIN_MAINNET: &[u8] =
    b"ZEBVIX_MAINNET_V1\x00";          // 17 ASCII bytes + NUL terminator

/// Off-chain domain tag for Zebvix **testnet** (chain_id 8990, public).
/// Signatures carrying this tag are invalid on devnet (and vice-versa)
/// even though both networks share chain_id 8990.
pub const SIGNING_DOMAIN_TESTNET: &[u8] =
    b"ZEBVIX_TESTNET_V1\x00";          // 17 ASCII bytes + NUL terminator

/// Off-chain domain tag for Zebvix **devnet** (chain_id 8990, internal).
/// Change this constant whenever devnet is hard-forked to reset its state,
/// to invalidate all previously issued off-chain signatures.
pub const SIGNING_DOMAIN_DEVNET: &[u8] =
    b"ZEBVIX_DEVNET_V1\x00";           // 16 ASCII bytes + NUL terminator

// ── SigningContext ─────────────────────────────────────────────────────────────

/// Immutable signing context for one chain.
#[derive(Debug, Clone)]
pub struct SigningContext {
    pub chain_id: u64,
    /// Default gas token for new transactions.
    pub default_gas_token: GasToken,
    /// Default max_priority_fee_per_gas (EIP-1559, in wei).
    pub default_priority_fee: u64,
    /// Default max_fee_per_gas (EIP-1559, in wei).
    pub default_max_fee: u64,
    /// Off-chain signing domain tag (P0-T07).
    /// Mixed into personal-sign message hashes to prevent devnet/testnet
    /// cross-replay.  Use `network_domain()` to access.
    network_domain: &'static [u8],
}

impl SigningContext {
    pub fn mainnet() -> Self {
        SigningContext {
            chain_id: CHAIN_ID_MAINNET,
            default_gas_token: GasToken::Zbx,
            default_priority_fee: 1_000_000_000,        // 1 Gwei
            default_max_fee: 10_000_000_000,             // 10 Gwei
            network_domain: SIGNING_DOMAIN_MAINNET,
        }
    }

    pub fn testnet() -> Self {
        SigningContext {
            chain_id: CHAIN_ID_TESTNET,
            default_gas_token: GasToken::Zbx,
            default_priority_fee: 1_000_000_000,
            default_max_fee: 10_000_000_000,
            network_domain: SIGNING_DOMAIN_TESTNET,
        }
    }

    /// Devnet context.  Shares `chain_id` with testnet (8990) but uses a
    /// distinct off-chain signing domain so paymaster / session-key / bridge
    /// relay signatures cannot be replayed across the two networks.
    pub fn devnet() -> Self {
        SigningContext {
            chain_id: CHAIN_ID_DEVNET,
            default_gas_token: GasToken::Zbx,
            default_priority_fee: 1_000_000_000,
            default_max_fee: 10_000_000_000,
            network_domain: SIGNING_DOMAIN_DEVNET,
        }
    }

    /// Returns the off-chain signing domain tag for this network context.
    ///
    /// Callers that produce or verify personal-sign messages (paymaster,
    /// session keys, bridge relay proofs) MUST prepend this tag to their
    /// application message before hashing.  See the module-level doc for
    /// the canonical usage pattern.
    pub fn network_domain(&self) -> &'static [u8] {
        self.network_domain
    }

    /// Sign a ZBX transfer (value send, no data).
    pub fn sign_transfer(
        &self,
        key: &PrivKey,
        from: Address,
        to: Address,
        value: u128,
        nonce: u64,
        gas_limit: u64,
    ) -> Result<SignedTx, TxError> {
        self.sign_raw(key, from, Some(to), value, nonce, gas_limit, vec![], vec![])
    }

    /// Sign a contract call (to = contract address, data = calldata).
    pub fn sign_contract_call(
        &self,
        key: &PrivKey,
        from: Address,
        to: Address,
        value: u128,
        nonce: u64,
        gas_limit: u64,
        data: Vec<u8>,
    ) -> Result<SignedTx, TxError> {
        self.sign_raw(key, from, Some(to), value, nonce, gas_limit, data, vec![])
    }

    /// Sign a contract deployment (to = None).
    pub fn sign_deploy(
        &self,
        key: &PrivKey,
        from: Address,
        value: u128,
        nonce: u64,
        gas_limit: u64,
        init_code: Vec<u8>,
    ) -> Result<SignedTx, TxError> {
        self.sign_raw(key, from, None, value, nonce, gas_limit, init_code, vec![])
    }

    /// Sign a contract call with an access list (EIP-2930 / Type-1).
    pub fn sign_with_access_list(
        &self,
        key: &PrivKey,
        from: Address,
        to: Option<Address>,
        value: u128,
        nonce: u64,
        gas_limit: u64,
        data: Vec<u8>,
        access_list: Vec<AccessListEntry>,
    ) -> Result<SignedTx, TxError> {
        let _ = from;
        let tx = Transaction {
            tx_type: TxType::Eip2930,
            chain_id: self.chain_id,
            nonce,
            max_priority_fee: 0,
            max_fee_per_gas: self.default_max_fee as u128,
            gas_limit,
            to: to.map(|a| a.0),
            value,
            data,
            access_list,
            gas_token: self.default_gas_token,
        };
        TxSigner::sign_transaction(tx, key)
    }

    /// Sign multiple transactions with sequential nonces starting at `base_nonce`.
    ///
    /// Useful for wallet batch operations — each tx increments the nonce by 1.
    /// Returns an error on the first signing failure; successful txs up to
    /// that point are returned in `ok`.
    pub fn batch_sign(
        &self,
        key: &PrivKey,
        from: Address,
        base_nonce: u64,
        requests: Vec<BatchSignRequest>,
    ) -> BatchSignResult {
        let mut ok = Vec::with_capacity(requests.len());
        let mut err = None;
        for (i, req) in requests.into_iter().enumerate() {
            let nonce = base_nonce + i as u64;
            let result = self.sign_raw(
                key,
                from.clone(),
                req.to,
                req.value,
                nonce,
                req.gas_limit,
                req.data,
                vec![],
            );
            match result {
                Ok(signed) => ok.push(signed),
                Err(e) => {
                    err = Some((nonce, e));
                    break;
                }
            }
        }
        BatchSignResult { signed: ok, error: err }
    }

    // ── Internal ─────────────────────────────────────────────────────────────

    fn sign_raw(
        &self,
        key: &PrivKey,
        from: Address,
        to: Option<Address>,
        value: u128,
        nonce: u64,
        gas_limit: u64,
        data: Vec<u8>,
        access_list: Vec<AccessListEntry>,
    ) -> Result<SignedTx, TxError> {
        let _ = from;
        let tx = Transaction {
            tx_type: TxType::Eip1559,
            chain_id: self.chain_id,
            nonce,
            max_priority_fee: self.default_priority_fee as u128,
            max_fee_per_gas: self.default_max_fee as u128,
            gas_limit,
            to: to.map(|a| a.0),
            value,
            data,
            access_list,
            gas_token: self.default_gas_token,
        };
        TxSigner::sign_transaction(tx, key)
    }
}

// ── Batch helpers ─────────────────────────────────────────────────────────────

/// One request in a batch signing call.
pub struct BatchSignRequest {
    pub to: Option<Address>,
    pub value: u128,
    pub gas_limit: u64,
    pub data: Vec<u8>,
}

/// Result of `batch_sign`.
pub struct BatchSignResult {
    /// Successfully signed transactions (in nonce order).
    pub signed: Vec<SignedTx>,
    /// The first failure: `(nonce, error)`.  `None` if all succeeded.
    pub error: Option<(u64, TxError)>,
}

impl BatchSignResult {
    pub fn all_ok(&self) -> bool { self.error.is_none() }
}

// ── Signature verification helpers ───────────────────────────────────────────

/// Recover the signer address from a raw signed EIP-2718 transaction.
///
/// Supports:
///   - EIP-1559 (type 2): `0x02 || RLP([chain_id, nonce, max_priority_fee, max_fee,
///     gas_limit, to, value, data, access_list, v, r, s])`
///   - EIP-2930 (type 1): `0x01 || RLP([chain_id, nonce, gas_price, gas_limit,
///     to, value, data, access_list, v, r, s])`
///   - Legacy (type 0):   `RLP([nonce, gas_price, gas_limit, to, value, data, v, r, s])`
///     with optional EIP-155 replay protection (`v = chain_id*2 + 35 + parity`).
pub fn recover_signer(raw_tx: &[u8]) -> Result<Address, TxError> {
    use sha3::{Digest, Keccak256};
    use zbx_crypto::secp256k1::{recover_signer as ec_recover, Signature};
    use zbx_types::H256;

    if raw_tx.is_empty() {
        return Err(TxError::InvalidSignature);
    }

    // ── Determine transaction type ────────────────────────────────────────────
    let (tx_type, rlp_bytes) = if raw_tx[0] >= 0xc0 {
        (0u8, raw_tx)           // Legacy — entire payload is an RLP list
    } else if raw_tx[0] == 0x01 || raw_tx[0] == 0x02 {
        (raw_tx[0], &raw_tx[1..]) // EIP-2930 or EIP-1559
    } else {
        return Err(TxError::Rlp(format!("unsupported tx type: 0x{:02x}", raw_tx[0])));
    };

    // ── RLP-decode the outer list into raw items (each item retains its header)
    let items = rlp_decode_list(rlp_bytes).map_err(TxError::Rlp)?;
    let n = items.len();
    if n < 3 {
        return Err(TxError::Rlp("tx has fewer than 3 fields".into()));
    }

    // ── Extract v, r, s (last 3 items) ───────────────────────────────────────
    let v_raw = &items[n - 3];
    let r_raw = &items[n - 2];
    let s_raw = &items[n - 1];

    let v_uint = rlp_decode_uint(v_raw);
    let recovery_id: u8 = if tx_type == 0 {
        // Legacy: v = 27 + parity (pre-EIP155) or v = chain_id*2 + 35 + parity
        if v_uint == 27 || v_uint == 28 {
            (v_uint - 27) as u8
        } else {
            (v_uint % 2) as u8  // EIP-155: strip chain_id contribution
        }
    } else {
        v_uint as u8  // typed txs: v is already 0 or 1
    };

    let r = rlp_decode_bytes_to_32(r_raw);
    let s = rlp_decode_bytes_to_32(s_raw);
    let mut sig_bytes = [0u8; 65];
    sig_bytes[..32].copy_from_slice(&r);
    sig_bytes[32..64].copy_from_slice(&s);
    sig_bytes[64] = recovery_id;
    let sig = Signature::from_bytes(&sig_bytes).map_err(|_| TxError::InvalidSignature)?;

    // ── Rebuild the signing preimage (all fields except v, r, s) ─────────────
    let preimage_rlp = rlp_encode_list(&items[..n - 3]);
    let preimage = if tx_type == 0 {
        preimage_rlp
    } else {
        let mut p = Vec::with_capacity(1 + preimage_rlp.len());
        p.push(tx_type);
        p.extend_from_slice(&preimage_rlp);
        p
    };

    // ── keccak256(preimage) → signing hash ───────────────────────────────────
    let hash_arr: [u8; 32] = Keccak256::digest(&preimage).into();
    let msg_hash = H256(hash_arr);

    ec_recover(&msg_hash, &sig).map_err(|_| TxError::InvalidSignature)
}

// ── Inline RLP helpers (transaction layer only) ───────────────────────────────

/// Decode an RLP list into its raw item encodings (each item includes its header).
fn rlp_decode_list(data: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    if data.is_empty() {
        return Ok(Vec::new());
    }
    let b = data[0];
    if b < 0xc0 {
        return Err(format!("expected RLP list, got 0x{b:02x}"));
    }
    let (payload, _) = rlp_list_payload(data)?;
    let mut items = Vec::new();
    let mut offset = 0;
    while offset < payload.len() {
        let sz = rlp_item_size(&payload[offset..])?;
        items.push(payload[offset..offset + sz].to_vec());
        offset += sz;
    }
    Ok(items)
}

fn rlp_list_payload(data: &[u8]) -> Result<(&[u8], usize), String> {
    let b = data[0];
    let (payload_len, header_len) = if b < 0xf8 {
        ((b - 0xc0) as usize, 1usize)
    } else {
        let ll = (b - 0xf7) as usize;
        if data.len() < 1 + ll {
            return Err("RLP list length truncated".into());
        }
        let mut len = 0usize;
        for &byte in &data[1..1 + ll] { len = (len << 8) | byte as usize; }
        (len, 1 + ll)
    };
    let end = header_len + payload_len;
    if data.len() < end { return Err("RLP list payload truncated".into()); }
    Ok((&data[header_len..end], end))
}

fn rlp_item_size(data: &[u8]) -> Result<usize, String> {
    if data.is_empty() { return Err("empty RLP item".into()); }
    let b = data[0];
    Ok(if b < 0x80 { 1 }
    else if b < 0xb8 { 1 + (b - 0x80) as usize }
    else if b < 0xc0 {
        let ll = (b - 0xb7) as usize;
        if data.len() < 1 + ll { return Err("RLP string length truncated".into()); }
        let mut len = 0usize;
        for &byte in &data[1..1 + ll] { len = (len << 8) | byte as usize; }
        1 + ll + len
    } else if b < 0xf8 { 1 + (b - 0xc0) as usize }
    else {
        let ll = (b - 0xf7) as usize;
        if data.len() < 1 + ll { return Err("RLP list length truncated".into()); }
        let mut len = 0usize;
        for &byte in &data[1..1 + ll] { len = (len << 8) | byte as usize; }
        1 + ll + len
    })
}

/// Re-encode a slice of already-encoded RLP items as a new RLP list.
fn rlp_encode_list(items: &[Vec<u8>]) -> Vec<u8> {
    let total: usize = items.iter().map(|i| i.len()).sum();
    let mut out = Vec::with_capacity(8 + total);
    if total < 56 {
        out.push(0xc0 + total as u8);
    } else {
        let lb = uint_to_bytes_be(total as u64);
        out.push(0xf7 + lb.len() as u8);
        out.extend_from_slice(&lb);
    }
    for item in items { out.extend_from_slice(item); }
    out
}

/// Decode a raw RLP integer item as u64 (for v field extraction).
fn rlp_decode_uint(raw: &[u8]) -> u64 {
    if raw.is_empty() { return 0; }
    let b = raw[0];
    if b < 0x80 { return b as u64; }
    if b < 0xb8 {
        let n = (b - 0x80) as usize;
        let end = (1 + n).min(raw.len());
        let payload = &raw[1..end];
        let mut val = 0u64;
        for &byte in payload { val = (val << 8) | byte as u64; }
        return val;
    }
    1 // oversized int — treat as non-zero
}

/// Decode a raw RLP bytes item (r or s) into a right-justified 32-byte array.
fn rlp_decode_bytes_to_32(raw: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    if raw.is_empty() { return out; }
    let payload = if raw[0] < 0x80 {
        raw
    } else if raw[0] < 0xb8 {
        let n = (raw[0] - 0x80) as usize;
        &raw[1..(1 + n).min(raw.len())]
    } else { return out; };
    let len = payload.len().min(32);
    out[32 - len..].copy_from_slice(&payload[..len]);
    out
}

fn uint_to_bytes_be(mut n: u64) -> Vec<u8> {
    let mut bytes = Vec::new();
    while n > 0 { bytes.insert(0, (n & 0xff) as u8); n >>= 8; }
    bytes
}

/// Verify that the signer of `raw_tx` matches `expected_from`.
pub fn verify_sender(raw_tx: &[u8], expected_from: &Address) -> Result<(), TxError> {
    let recovered = recover_signer(raw_tx)?;
    if &recovered != expected_from {
        return Err(TxError::InvalidSignature);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signer::TxSigner;
    use crate::types::{Transaction, TxType, GasToken};
    use zbx_crypto::secp256k1::PrivKey;

    fn make_eip1559_raw(chain_id: u64, privkey: &PrivKey) -> Vec<u8> {
        let tx = Transaction {
            tx_type: TxType::Eip1559,
            chain_id,
            nonce: 42,
            max_fee_per_gas: 20_000_000_000,
            max_priority_fee: 1_000_000_000,
            gas_limit: 21_000,
            to: Some([0xab; 20]),
            value: 1_000_000_000_000_000_000,
            data: vec![],
            access_list: vec![],
            gas_token: GasToken::Zbx,
        };
        let signed = TxSigner::sign_transaction(tx, privkey).unwrap();
        TxSigner::encode_signed_tx(&signed)
    }

    fn make_legacy_raw(chain_id: u64, privkey: &PrivKey) -> Vec<u8> {
        let tx = Transaction {
            tx_type: TxType::Legacy,
            chain_id,
            nonce: 1,
            max_fee_per_gas: 10_000_000_000,
            max_priority_fee: 0,
            gas_limit: 21_000,
            to: Some([0xcd; 20]),
            value: 500_000_000_000_000_000,
            data: vec![],
            access_list: vec![],
            gas_token: GasToken::Zbx,
        };
        let signed = TxSigner::sign_transaction(tx, privkey).unwrap();
        TxSigner::encode_signed_tx(&signed)
    }

    fn make_eip2930_raw(chain_id: u64, privkey: &PrivKey) -> Vec<u8> {
        let tx = Transaction {
            tx_type: TxType::Eip2930,
            chain_id,
            nonce: 7,
            max_fee_per_gas: 5_000_000_000,
            max_priority_fee: 0,
            gas_limit: 30_000,
            to: Some([0xef; 20]),
            value: 0,
            data: vec![0xde, 0xad, 0xbe, 0xef],
            access_list: vec![],
            gas_token: GasToken::Zbx,
        };
        let signed = TxSigner::sign_transaction(tx, privkey).unwrap();
        TxSigner::encode_signed_tx(&signed)
    }

    // ── recover_signer round-trip tests ─────────────────────────────────────

    #[test]
    fn recover_signer_eip1559_round_trip() {
        let privkey = PrivKey::random();
        let expected = privkey.to_address();
        let raw = make_eip1559_raw(8989, &privkey);
        let recovered = recover_signer(&raw).unwrap();
        assert_eq!(recovered, expected, "EIP-1559: recovered address must match signer");
    }

    #[test]
    fn recover_signer_legacy_round_trip() {
        let privkey = PrivKey::random();
        let expected = privkey.to_address();
        let raw = make_legacy_raw(8989, &privkey);
        let recovered = recover_signer(&raw).unwrap();
        assert_eq!(recovered, expected, "Legacy EIP-155: recovered address must match signer");
    }

    #[test]
    fn recover_signer_eip2930_round_trip() {
        let privkey = PrivKey::random();
        let expected = privkey.to_address();
        let raw = make_eip2930_raw(8989, &privkey);
        let recovered = recover_signer(&raw).unwrap();
        assert_eq!(recovered, expected, "EIP-2930: recovered address must match signer");
    }

    #[test]
    fn recover_signer_eip1559_multiple_chain_ids() {
        let privkey = PrivKey::random();
        let expected = privkey.to_address();
        for chain_id in [1u64, 5, 137, 8989, 8990] {
            let raw = make_eip1559_raw(chain_id, &privkey);
            let recovered = recover_signer(&raw).unwrap();
            assert_eq!(recovered, expected, "chain_id={chain_id}: wrong signer recovered");
        }
    }

    #[test]
    fn recover_signer_two_different_keys_produce_different_addresses() {
        let pk1 = PrivKey::random();
        let pk2 = PrivKey::random();
        let raw1 = make_eip1559_raw(8989, &pk1);
        let raw2 = make_eip1559_raw(8989, &pk2);
        let r1 = recover_signer(&raw1).unwrap();
        let r2 = recover_signer(&raw2).unwrap();
        assert_ne!(r1, r2, "two different keys must recover to different addresses");
    }

    // ── tamper / error path tests ────────────────────────────────────────────

    #[test]
    fn recover_signer_empty_input_errors() {
        assert!(recover_signer(&[]).is_err());
    }

    #[test]
    fn recover_signer_truncated_input_errors() {
        let privkey = PrivKey::random();
        let raw = make_eip1559_raw(8989, &privkey);
        // A tx truncated to half its length is invalid
        assert!(recover_signer(&raw[..raw.len() / 2]).is_err());
    }

    #[test]
    fn recover_signer_unknown_type_byte_errors() {
        // Type byte 0x03 is not a defined EIP-2718 type
        let garbage = vec![0x03u8; 64];
        assert!(recover_signer(&garbage).is_err());
    }

    #[test]
    fn recover_signer_tampered_sig_does_not_match_original_signer() {
        let privkey = PrivKey::random();
        let expected = privkey.to_address();
        let mut raw = make_eip1559_raw(8989, &privkey);
        // Corrupt the last 35 bytes (deep inside the s/r fields in the RLP)
        let len = raw.len();
        raw[len - 35] ^= 0xff;
        // Must either error or recover a *different* address — never the original one
        match recover_signer(&raw) {
            Ok(addr) => assert_ne!(addr, expected, "tampered tx must not recover original signer"),
            Err(_) => {}
        }
    }

    // ── verify_sender tests ──────────────────────────────────────────────────

    #[test]
    fn verify_sender_accepts_correct_address() {
        let privkey = PrivKey::random();
        let addr = privkey.to_address();
        let raw = make_eip1559_raw(8989, &privkey);
        verify_sender(&raw, &addr).expect("verify_sender must accept the correct signer address");
    }

    #[test]
    fn verify_sender_rejects_wrong_address() {
        let privkey = PrivKey::random();
        let raw = make_eip1559_raw(8989, &privkey);
        let other_addr = PrivKey::random().to_address();
        assert!(
            verify_sender(&raw, &other_addr).is_err(),
            "verify_sender must reject a different address"
        );
    }

    #[test]
    fn verify_sender_legacy_accepts_correct_address() {
        let privkey = PrivKey::random();
        let addr = privkey.to_address();
        let raw = make_legacy_raw(8989, &privkey);
        verify_sender(&raw, &addr).expect("verify_sender must accept legacy signer");
    }
}
