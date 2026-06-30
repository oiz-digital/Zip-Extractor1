//! Transaction signing and sender recovery — real ECDSA via k256.
//!
//! ## Signing hash computation
//!
//! * **EIP-1559 (Type 2):** `keccak256(0x02 || RLP([chain_id, nonce,
//!   max_priority_fee, max_fee, gas_limit, to, value, data, access_list,
//!   gas_token]))`
//!   `gas_token` is a ZBX-native extension (0=ZBX, 1=ZUSD).
//!   It is always included so it cannot be changed after signing.
//!   Legacy tools that don't set gas_token get the default (0 = ZBX).
//! * **EIP-2930 (Type 1):** `keccak256(0x01 || RLP([chain_id, nonce,
//!   gas_price, gas_limit, to, value, data, access_list]))`
//! * **Legacy + EIP-155:** `keccak256(RLP([nonce, gas_price, gas_limit, to,
//!   value, data, chain_id, 0, 0]))`
//! * **Legacy pre-EIP-155 (v=27/28):** `keccak256(RLP([nonce, gas_price,
//!   gas_limit, to, value, data]))`
//!
//! ## Signed transaction hash
//!
//! The "transaction hash" used by `eth_getTransactionByHash` is NOT the signing
//! hash. It is `keccak256(EIP-2718 encoded signed transaction)` where the
//! encoding includes the (v, r, s) signature components. Use
//! `TxSigner::signed_tx_hash` or `TxSigner::sign_transaction` (which sets
//! `SignedTx.hash` automatically) to obtain the correct transaction hash.
//!
//! ## EIP-2718 broadcast encoding
//!
//! `TxSigner::encode_signed_tx` returns the raw bytes suitable for
//! `eth_sendRawTransaction`. For typed transactions, this is the
//! `type_byte || RLP(fields_with_sig)` envelope. For legacy, plain RLP.

use crate::types::{AccessListEntry, GasToken, SignedTx, Transaction, TxType};
use crate::TxError;
use k256::ecdsa::{RecoveryId, Signature as KSig, VerifyingKey};
use sha3::{Digest, Keccak256};
use zbx_crypto::secp256k1::PrivKey;
use zbx_types::H256;

// ---------------------------------------------------------------------------
// EIP-2 — High-S signature malleability rejection
// ---------------------------------------------------------------------------

/// Half of the secp256k1 curve order (big-endian).
///
/// n/2 = 0x7FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF5D576E7357A4501DDFE92F46681B20A0
///
/// EIP-2 (Homestead) mandates that the `s` component of every transaction
/// signature satisfies `s ≤ HALF_CURVE_ORDER`.  A high-S signature (s > n/2)
/// is mathematically equivalent to a low-S signature with the opposite parity
/// (`s' = n - s`, `v' = 1 - v`), so accepting high-S creates signature
/// malleability: any third party can transform a valid signature into a
/// second valid signature without the private key, producing a different
/// transaction hash for the same semantic transaction.
///
/// Ethereum has enforced low-S since block 2,675,000 (EIP-2).  ZBX Chain
/// enforces it at validation time (TX-SEC-01).
const HALF_CURVE_ORDER: [u8; 32] = [
    0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0x5d, 0x57, 0x6e, 0x73, 0x57, 0xa4, 0x50, 0x1d,
    0xdf, 0xe9, 0x2f, 0x46, 0x68, 0x1b, 0x20, 0xa0,
];

/// Returns `true` when `s` is in the lower half of the secp256k1 curve order.
///
/// Both `s` and `HALF_CURVE_ORDER` are 32-byte big-endian values, so a
/// lexicographic comparison is equivalent to a numeric comparison.
#[inline]
fn is_low_s(s: &[u8; 32]) -> bool {
    s.as_ref() <= HALF_CURVE_ORDER.as_ref()
}

/// Signs transactions and recovers senders via secp256k1 ECDSA.
pub struct TxSigner;

impl TxSigner {
    /// Recover the sender address from a signed transaction.
    ///
    /// Fills in `tx.from` on success.
    pub fn recover_sender(tx: &mut SignedTx) -> Result<[u8; 20], TxError> {
        let hash = signing_hash(tx).ok_or(TxError::InvalidSignature)?;
        let parity = normalize_v(tx).ok_or(TxError::InvalidSignature)?;
        let addr = recover_from_hash(&hash, parity, &tx.r, &tx.s)
            .ok_or(TxError::InvalidSignature)?;
        tx.from = Some(addr);
        Ok(addr)
    }

    /// Sign a transaction with the given private key and return a fully formed
    /// `SignedTx` with correct `from` address and `hash` field.
    ///
    /// All signing is done via RFC 6979 deterministic nonces with mandatory
    /// low-S enforcement (EIP-2). Legacy transactions are always signed with
    /// EIP-155 chain replay protection.
    ///
    /// The returned `SignedTx.hash` is the real EIP-2718 transaction hash
    /// (`keccak256` of the broadcast encoding), suitable for use with
    /// `eth_getTransactionByHash`.
    pub fn sign_transaction(tx: Transaction, privkey: &PrivKey) -> Result<SignedTx, TxError> {
        // Build a temporary SignedTx so we can reuse the existing signing_hash logic.
        // For Legacy, use EIP-155 signing (chain replay protection always on).
        let signing_v = match tx.tx_type {
            TxType::Eip1559 | TxType::Eip2930 => 0u64,
            TxType::Legacy => tx.chain_id * 2 + 35, // EIP-155, parity=0 for the signing hash input
        };
        let temp = SignedTx {
            tx: tx.clone(),
            v: signing_v,
            r: [0u8; 32],
            s: [0u8; 32],
            from: None,
            hash: [0u8; 32],
        };
        let hash_bytes = signing_hash(&temp).ok_or(TxError::InvalidSignature)?;
        let hash = H256(hash_bytes);
        let sig = privkey.sign(&hash);

        // Map the low-S ECDSA parity (0 or 1) to the on-wire `v` value.
        let v = match tx.tx_type {
            TxType::Eip1559 | TxType::Eip2930 => sig.v as u64, // signature_y_parity: 0 or 1
            TxType::Legacy => tx.chain_id * 2 + 35 + sig.v as u64, // EIP-155 encoded
        };

        let from = privkey.to_address().0;
        let mut signed = SignedTx {
            tx,
            v,
            r: sig.r.0,
            s: sig.s.0,
            from: Some(from),
            hash: [0u8; 32],
        };
        // Compute and store the real EIP-2718 transaction hash.
        signed.hash = Self::signed_tx_hash(&signed);
        Ok(signed)
    }

    /// Compute the EIP-2718 transaction hash for a fully signed transaction.
    ///
    /// `hash = keccak256(encode_signed_tx(signed))`
    ///
    /// This is the hash used by `eth_getTransactionByHash`. It REQUIRES the
    /// signature (v, r, s) — it is NOT the same as the signing hash.
    pub fn signed_tx_hash(signed: &SignedTx) -> [u8; 32] {
        keccak256_bytes(&Self::encode_signed_tx(signed))
    }

    /// Encode a signed transaction to its EIP-2718 wire format.
    ///
    /// Used for `eth_sendRawTransaction` and for computing the transaction hash.
    ///
    /// * **Type 2 (EIP-1559):** `0x02 || RLP([chain_id, nonce, max_priority_fee,
    ///   max_fee, gas_limit, to, value, data, access_list, gas_token, v, r, s])`
    /// * **Type 1 (EIP-2930):** `0x01 || RLP([chain_id, nonce, gas_price,
    ///   gas_limit, to, value, data, access_list, v, r, s])`
    /// * **Type 0 (Legacy):** `RLP([nonce, gas_price, gas_limit, to, value, data,
    ///   v, r, s])` where `v` is the EIP-155 encoded value.
    pub fn encode_signed_tx(signed: &SignedTx) -> Vec<u8> {
        match signed.tx.tx_type {
            TxType::Eip1559 => {
                let payload = rlp_list_from_items(vec![
                    rlp_u64(signed.tx.chain_id),
                    rlp_u64(signed.tx.nonce),
                    rlp_u128(signed.tx.max_priority_fee),
                    rlp_u128(signed.tx.max_fee_per_gas),
                    rlp_u64(signed.tx.gas_limit),
                    rlp_to(signed.tx.to),
                    rlp_u128(signed.tx.value),
                    rlp_bytes(&signed.tx.data),
                    rlp_access_list(&signed.tx.access_list),
                    rlp_u64(signed.tx.gas_token as u64), // ZBX extension
                    rlp_u64(signed.v),                   // signature_y_parity (0 or 1)
                    rlp_signature_scalar(&signed.r),
                    rlp_signature_scalar(&signed.s),
                ]);
                let mut out = Vec::with_capacity(1 + payload.len());
                out.push(0x02);
                out.extend_from_slice(&payload);
                out
            }
            TxType::Eip2930 => {
                let payload = rlp_list_from_items(vec![
                    rlp_u64(signed.tx.chain_id),
                    rlp_u64(signed.tx.nonce),
                    rlp_u128(signed.tx.max_fee_per_gas), // gas_price
                    rlp_u64(signed.tx.gas_limit),
                    rlp_to(signed.tx.to),
                    rlp_u128(signed.tx.value),
                    rlp_bytes(&signed.tx.data),
                    rlp_access_list(&signed.tx.access_list),
                    rlp_u64(signed.v),                   // signature_y_parity (0 or 1)
                    rlp_signature_scalar(&signed.r),
                    rlp_signature_scalar(&signed.s),
                ]);
                let mut out = Vec::with_capacity(1 + payload.len());
                out.push(0x01);
                out.extend_from_slice(&payload);
                out
            }
            TxType::Legacy => {
                // Legacy: RLP([nonce, gas_price, gas_limit, to, value, data, v, r, s])
                // v is already the EIP-155 encoded value (chain_id*2+35+parity) or 27/28.
                rlp_list_from_items(vec![
                    rlp_u64(signed.tx.nonce),
                    rlp_u128(signed.tx.max_fee_per_gas),
                    rlp_u64(signed.tx.gas_limit),
                    rlp_to(signed.tx.to),
                    rlp_u128(signed.tx.value),
                    rlp_bytes(&signed.tx.data),
                    rlp_u64(signed.v),
                    rlp_signature_scalar(&signed.r),
                    rlp_signature_scalar(&signed.s),
                ])
            }
        }
    }

    /// Compute the signing hash for an unsigned transaction (for display /
    /// hardware-wallet integration). This is NOT the transaction hash.
    ///
    /// For the real transaction hash (after signing), use `signed_tx_hash`.
    ///
    /// Legacy transactions are treated as EIP-155 (chain_id embedded).
    pub fn unsigned_signing_hash(tx: &Transaction) -> [u8; 32] {
        match tx.tx_type {
            TxType::Eip1559 => {
                let payload = rlp_eip1559(tx);
                let mut prefixed = Vec::with_capacity(1 + payload.len());
                prefixed.push(0x02);
                prefixed.extend_from_slice(&payload);
                keccak256_bytes(&prefixed)
            }
            TxType::Eip2930 => {
                let payload = rlp_eip2930(tx);
                let mut prefixed = Vec::with_capacity(1 + payload.len());
                prefixed.push(0x01);
                prefixed.extend_from_slice(&payload);
                keccak256_bytes(&prefixed)
            }
            TxType::Legacy => {
                // Always use EIP-155 for unsigned hash display.
                keccak256_bytes(&rlp_legacy_eip155(tx, tx.chain_id))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Signing hash
// ---------------------------------------------------------------------------

/// Compute the 32-byte hash that the sender signed for the given `SignedTx`.
pub fn signing_hash(signed_tx: &SignedTx) -> Option<[u8; 32]> {
    let tx = &signed_tx.tx;
    match tx.tx_type {
        TxType::Eip1559 => {
            let payload = rlp_eip1559(tx);
            let mut prefixed = Vec::with_capacity(1 + payload.len());
            prefixed.push(0x02);
            prefixed.extend_from_slice(&payload);
            Some(keccak256_bytes(&prefixed))
        }
        TxType::Eip2930 => {
            let payload = rlp_eip2930(tx);
            let mut prefixed = Vec::with_capacity(1 + payload.len());
            prefixed.push(0x01);
            prefixed.extend_from_slice(&payload);
            Some(keccak256_bytes(&prefixed))
        }
        TxType::Legacy => {
            let v = signed_tx.v;
            if v == 27 || v == 28 {
                // Pre-EIP-155: 6-field RLP
                Some(keccak256_bytes(&rlp_legacy_no_chain(tx)))
            } else if v >= 35 {
                // EIP-155: derive chain_id from v = chain_id*2 + 35 + parity
                let parity = ((v - 35) & 1) as u8;
                let chain_id = (v - 35 - parity as u64) / 2;
                Some(keccak256_bytes(&rlp_legacy_eip155(tx, chain_id)))
            } else {
                None
            }
        }
    }
}

// ---------------------------------------------------------------------------
// V normalization
// ---------------------------------------------------------------------------

/// Normalize the `v` field to a canonical ECDSA recovery id {0, 1}.
fn normalize_v(signed_tx: &SignedTx) -> Option<u8> {
    let v = signed_tx.v;
    match signed_tx.tx.tx_type {
        TxType::Eip1559 | TxType::Eip2930 => {
            // Typed txs: signature_y_parity is directly 0 or 1.
            if v <= 1 { Some(v as u8) } else { None }
        }
        TxType::Legacy => {
            if v == 27 || v == 28 {
                Some((v - 27) as u8)
            } else if v >= 35 {
                Some(((v - 35) & 1) as u8)
            } else {
                None
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ECDSA recovery
// ---------------------------------------------------------------------------

fn recover_from_hash(hash: &[u8; 32], parity: u8, r: &[u8; 32], s: &[u8; 32]) -> Option<[u8; 20]> {
    // TX-SEC-01 (EIP-2): Reject high-S signatures before any further processing.
    // High-S allows any observer to produce an alternative valid (r, n-s, 1-parity)
    // signature without the private key — creating a second valid tx hash for the
    // same sender intent (signature malleability).
    if !is_low_s(s) {
        return None;
    }

    // Build 64-byte compact r || s representation.
    let mut rs = [0u8; 64];
    rs[..32].copy_from_slice(r);
    rs[32..64].copy_from_slice(s);

    let ksig = KSig::from_slice(&rs).ok()?;
    let recid = RecoveryId::try_from(parity).ok()?;
    let vk = VerifyingKey::recover_from_prehash(hash, &ksig, recid).ok()?;

    // Uncompressed SEC1 public key: 0x04 || X (32 bytes) || Y (32 bytes).
    let enc = vk.to_encoded_point(false);
    let bytes = enc.as_bytes();
    if bytes.len() != 65 {
        return None;
    }
    // EVM address = last 20 bytes of keccak256(pubkey[1..]).
    let h = keccak256_bytes(&bytes[1..]);
    let mut addr = [0u8; 20];
    addr.copy_from_slice(&h[12..]);
    Some(addr)
}

// ---------------------------------------------------------------------------
// Inline minimal RLP encoder
// We avoid importing zbx-rlp here to keep this crate self-contained and avoid
// a potential dependency cycle. The encoding surface is narrow and stable.
// ---------------------------------------------------------------------------

fn keccak256_bytes(data: &[u8]) -> [u8; 32] {
    let mut h = Keccak256::new();
    h.update(data);
    h.finalize().into()
}

/// EIP-1559 signing payload (ZBX extension):
/// RLP([chain_id, nonce, max_priority_fee, max_fee, gas_limit, to, value,
///      data, access_list, gas_token])
///
/// `gas_token` (0=ZBX, 1=ZUSD) is a ZBX-native extension appended
/// as the final field. Legacy tools always produce gas_token=0 (ZBX default),
/// so they remain compatible with this format.
fn rlp_eip1559(tx: &Transaction) -> Vec<u8> {
    rlp_list_from_items(vec![
        rlp_u64(tx.chain_id),
        rlp_u64(tx.nonce),
        rlp_u128(tx.max_priority_fee),
        rlp_u128(tx.max_fee_per_gas),
        rlp_u64(tx.gas_limit),
        rlp_to(tx.to),
        rlp_u128(tx.value),
        rlp_bytes(&tx.data),
        rlp_access_list(&tx.access_list),
        // ZBX extension: covers the gas token in the signature.
        rlp_u64(tx.gas_token as u64),
    ])
}

/// EIP-2930 signing payload: RLP([chain_id, nonce, gas_price, gas_limit, to,
/// value, data, access_list]). Uses max_fee_per_gas as gas_price.
fn rlp_eip2930(tx: &Transaction) -> Vec<u8> {
    rlp_list_from_items(vec![
        rlp_u64(tx.chain_id),
        rlp_u64(tx.nonce),
        rlp_u128(tx.max_fee_per_gas),
        rlp_u64(tx.gas_limit),
        rlp_to(tx.to),
        rlp_u128(tx.value),
        rlp_bytes(&tx.data),
        rlp_access_list(&tx.access_list),
    ])
}

/// Legacy signing payload without EIP-155 (pre-EIP-155, 6 fields).
fn rlp_legacy_no_chain(tx: &Transaction) -> Vec<u8> {
    rlp_list_from_items(vec![
        rlp_u64(tx.nonce),
        rlp_u128(tx.max_fee_per_gas),
        rlp_u64(tx.gas_limit),
        rlp_to(tx.to),
        rlp_u128(tx.value),
        rlp_bytes(&tx.data),
    ])
}

/// Legacy EIP-155 signing payload: RLP([nonce, gas_price, gas_limit, to,
/// value, data, chain_id, 0, 0]).
fn rlp_legacy_eip155(tx: &Transaction, chain_id: u64) -> Vec<u8> {
    rlp_list_from_items(vec![
        rlp_u64(tx.nonce),
        rlp_u128(tx.max_fee_per_gas),
        rlp_u64(tx.gas_limit),
        rlp_to(tx.to),
        rlp_u128(tx.value),
        rlp_bytes(&tx.data),
        rlp_u64(chain_id),
        rlp_bytes(&[]),
        rlp_bytes(&[]),
    ])
}

/// Encode the `to` field: None = empty (contract creation), Some = 20-byte addr.
fn rlp_to(to: Option<[u8; 20]>) -> Vec<u8> {
    match to {
        None => vec![0x80],
        Some(addr) => rlp_bytes(&addr),
    }
}

/// Encode an EVM access list as an RLP list of [address, [storage_key...]] pairs.
fn rlp_access_list(list: &[AccessListEntry]) -> Vec<u8> {
    let entries: Vec<Vec<u8>> = list
        .iter()
        .map(|e| {
            let addr_item = rlp_bytes(&e.address);
            let keys_list = rlp_list_from_items(
                e.storage_keys.iter().map(|k| rlp_bytes(k)).collect(),
            );
            rlp_list_from_items(vec![addr_item, keys_list])
        })
        .collect();
    rlp_list_from_items(entries)
}

/// RLP-encode a u64, stripping leading zero bytes (minimal encoding).
fn rlp_u64(v: u64) -> Vec<u8> {
    if v == 0 {
        return vec![0x80];
    }
    let bytes = v.to_be_bytes();
    let skip = bytes.iter().take_while(|&&b| b == 0).count();
    rlp_bytes(&bytes[skip..])
}

/// RLP-encode a u128, stripping leading zero bytes.
fn rlp_u128(v: u128) -> Vec<u8> {
    if v == 0 {
        return vec![0x80];
    }
    let bytes = v.to_be_bytes();
    let skip = bytes.iter().take_while(|&&b| b == 0).count();
    rlp_bytes(&bytes[skip..])
}

/// RLP-encode a 32-byte ECDSA scalar (r or s) as a big-integer.
///
/// Strips leading zero bytes for minimal encoding, matching Ethereum's
/// convention for signature scalars in signed transaction RLP.
fn rlp_signature_scalar(v: &[u8; 32]) -> Vec<u8> {
    let skip = v.iter().take_while(|&&b| b == 0).count();
    if skip == 32 {
        return vec![0x80]; // zero scalar encodes as empty byte string
    }
    rlp_bytes(&v[skip..])
}

/// RLP-encode a raw byte string.
fn rlp_bytes(data: &[u8]) -> Vec<u8> {
    match data.len() {
        0 => vec![0x80],
        1 if data[0] < 0x80 => vec![data[0]],
        n if n <= 55 => {
            let mut out = Vec::with_capacity(1 + n);
            out.push(0x80 + n as u8);
            out.extend_from_slice(data);
            out
        }
        n => {
            let len_b = minimal_be_bytes(n as u64);
            let mut out = Vec::with_capacity(1 + len_b.len() + n);
            out.push(0xb7 + len_b.len() as u8);
            out.extend_from_slice(&len_b);
            out.extend_from_slice(data);
            out
        }
    }
}

/// Wrap a list of pre-encoded RLP items into an RLP list encoding.
fn rlp_list_from_items(items: Vec<Vec<u8>>) -> Vec<u8> {
    let payload: Vec<u8> = items.into_iter().flatten().collect();
    let n = payload.len();
    if n <= 55 {
        let mut out = Vec::with_capacity(1 + n);
        out.push(0xc0 + n as u8);
        out.extend_from_slice(&payload);
        out
    } else {
        let len_b = minimal_be_bytes(n as u64);
        let mut out = Vec::with_capacity(1 + len_b.len() + n);
        out.push(0xf7 + len_b.len() as u8);
        out.extend_from_slice(&len_b);
        out.extend_from_slice(&payload);
        out
    }
}

fn minimal_be_bytes(v: u64) -> Vec<u8> {
    let bytes = v.to_be_bytes();
    let skip = bytes.iter().take_while(|&&b| b == 0).count();
    bytes[skip..].to_vec()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{GasToken, SignedTx, Transaction, TxType};

    fn make_eip1559_signed(chain_id: u64) -> SignedTx {
        let tx = Transaction {
            tx_type: TxType::Eip1559,
            chain_id,
            nonce: 1,
            max_fee_per_gas: 20_000_000_000,
            max_priority_fee: 1_000_000_000,
            gas_limit: 21_000,
            to: Some([0xde; 20]),
            value: 1_000_000_000_000_000_000,
            data: vec![],
            access_list: vec![],
            gas_token: GasToken::Zbx,
        };
        SignedTx {
            tx,
            v: 0,
            r: [1u8; 32],
            s: [2u8; 32],
            from: None,
            hash: [0u8; 32],
        }
    }

    #[test]
    fn signing_hash_eip1559_is_deterministic() {
        let stx = make_eip1559_signed(8989);
        let h1 = signing_hash(&stx).unwrap();
        let h2 = signing_hash(&stx).unwrap();
        assert_eq!(h1, h2);
        assert_ne!(h1, [0u8; 32]);
    }

    #[test]
    fn signing_hash_differs_by_chain_id() {
        let stx1 = make_eip1559_signed(8989);
        let stx2 = make_eip1559_signed(1);
        let h1 = signing_hash(&stx1).unwrap();
        let h2 = signing_hash(&stx2).unwrap();
        assert_ne!(h1, h2, "different chain_ids must produce different signing hashes");
    }

    #[test]
    fn normalize_v_eip1559() {
        let mut stx = make_eip1559_signed(8989);
        stx.v = 0;
        assert_eq!(normalize_v(&stx), Some(0));
        stx.v = 1;
        assert_eq!(normalize_v(&stx), Some(1));
        stx.v = 2;
        assert_eq!(normalize_v(&stx), None);
    }

    #[test]
    fn normalize_v_legacy_pre_eip155() {
        let mut stx = make_eip1559_signed(8989);
        stx.tx.tx_type = TxType::Legacy;
        stx.v = 27;
        assert_eq!(normalize_v(&stx), Some(0));
        stx.v = 28;
        assert_eq!(normalize_v(&stx), Some(1));
    }

    #[test]
    fn normalize_v_legacy_eip155_zbx() {
        // ZBX Chain: chain_id=8989, v = 8989*2 + 35 + parity
        let mut stx = make_eip1559_signed(8989);
        stx.tx.tx_type = TxType::Legacy;
        stx.v = 8989 * 2 + 35; // parity=0
        assert_eq!(normalize_v(&stx), Some(0));
        stx.v = 8989 * 2 + 36; // parity=1
        assert_eq!(normalize_v(&stx), Some(1));
    }

    #[test]
    fn sign_and_recover_eip1559() {
        let privkey = PrivKey::random();
        let addr = privkey.to_address();
        let tx = Transaction {
            tx_type: TxType::Eip1559,
            chain_id: 8989,
            nonce: 0,
            max_fee_per_gas: 1_000_000_000,
            max_priority_fee: 100_000_000,
            gas_limit: 21_000,
            to: Some([0xab; 20]),
            value: 1_000_000_000_000_000_000u128,
            data: vec![],
            access_list: vec![],
            gas_token: GasToken::Zbx,
        };
        let mut signed = TxSigner::sign_transaction(tx, &privkey).unwrap();
        assert_eq!(signed.from, Some(addr.0), "from must match signer");
        assert_ne!(signed.hash, [0u8; 32], "hash must be non-zero");
        // The hash must be the keccak256 of the encoded signed tx
        let recomputed = TxSigner::signed_tx_hash(&signed);
        assert_eq!(signed.hash, recomputed, "hash must equal keccak256(encode_signed_tx)");
        // Recovery must succeed and match
        let recovered = TxSigner::recover_sender(&mut signed).unwrap();
        assert_eq!(recovered, addr.0, "recovered sender must match signer");
    }

    #[test]
    fn sign_and_recover_legacy_eip155() {
        let privkey = PrivKey::random();
        let addr = privkey.to_address();
        let tx = Transaction {
            tx_type: TxType::Legacy,
            chain_id: 8989,
            nonce: 5,
            max_fee_per_gas: 20_000_000_000,
            max_priority_fee: 0,
            gas_limit: 21_000,
            to: Some([0xcd; 20]),
            value: 0,
            data: vec![],
            access_list: vec![],
            gas_token: GasToken::Zbx,
        };
        let mut signed = TxSigner::sign_transaction(tx, &privkey).unwrap();
        // EIP-155 v: chain_id*2+35+parity
        assert!(signed.v >= 8989 * 2 + 35, "v must be EIP-155 encoded");
        let recovered = TxSigner::recover_sender(&mut signed).unwrap();
        assert_eq!(recovered, addr.0, "legacy EIP-155 recovery must match signer");
    }

    #[test]
    fn sign_and_recover_eip2930() {
        let privkey = PrivKey::random();
        let addr = privkey.to_address();
        let tx = Transaction {
            tx_type: TxType::Eip2930,
            chain_id: 8989,
            nonce: 2,
            max_fee_per_gas: 5_000_000_000,
            max_priority_fee: 0,
            gas_limit: 30_000,
            to: Some([0xef; 20]),
            value: 0,
            data: vec![0xde, 0xad],
            access_list: vec![],
            gas_token: GasToken::Zbx,
        };
        let mut signed = TxSigner::sign_transaction(tx, &privkey).unwrap();
        assert!(signed.v <= 1, "EIP-2930 v must be 0 or 1");
        let recovered = TxSigner::recover_sender(&mut signed).unwrap();
        assert_eq!(recovered, addr.0, "EIP-2930 recovery must match signer");
    }

    #[test]
    fn signed_tx_hash_differs_from_signing_hash() {
        let privkey = PrivKey::random();
        let tx = Transaction {
            tx_type: TxType::Eip1559,
            chain_id: 8989,
            nonce: 0,
            max_fee_per_gas: 1_000_000_000,
            max_priority_fee: 100_000_000,
            gas_limit: 21_000,
            to: Some([0x11; 20]),
            value: 0,
            data: vec![],
            access_list: vec![],
            gas_token: GasToken::Zbx,
        };
        let signing_h = TxSigner::unsigned_signing_hash(&tx);
        let signed = TxSigner::sign_transaction(tx, &privkey).unwrap();
        // The transaction hash MUST differ from the signing hash (it includes v/r/s).
        assert_ne!(
            signed.hash, signing_h,
            "tx hash must differ from signing hash — it includes the signature"
        );
    }

    #[test]
    fn encode_decode_roundtrip_deterministic() {
        let privkey = PrivKey::random();
        let tx = Transaction {
            tx_type: TxType::Eip1559,
            chain_id: 8989,
            nonce: 42,
            max_fee_per_gas: 3_000_000_000,
            max_priority_fee: 1_000_000_000,
            gas_limit: 21_000,
            to: Some([0x99; 20]),
            value: 1_000,
            data: vec![],
            access_list: vec![],
            gas_token: GasToken::Zbx,
        };
        let signed1 = TxSigner::sign_transaction(tx.clone(), &privkey).unwrap();
        let signed2 = TxSigner::sign_transaction(tx, &privkey).unwrap();
        // RFC 6979 deterministic nonce means identical inputs produce identical output.
        assert_eq!(signed1.hash, signed2.hash, "RFC 6979 must produce deterministic signatures");
        assert_eq!(signed1.r, signed2.r);
        assert_eq!(signed1.s, signed2.s);
    }

    #[test]
    fn gas_token_zusd_covered_by_signature() {
        let privkey = PrivKey::random();
        let mut tx_zbx = Transaction {
            tx_type: TxType::Eip1559,
            chain_id: 8989,
            nonce: 0,
            max_fee_per_gas: 1_000_000_000,
            max_priority_fee: 100_000_000,
            gas_limit: 21_000,
            to: Some([0x11; 20]),
            value: 0,
            data: vec![],
            access_list: vec![],
            gas_token: GasToken::Zbx,
        };
        let mut tx_zusd = tx_zbx.clone();
        tx_zusd.gas_token = GasToken::Zusd;
        let signed_zbx = TxSigner::sign_transaction(tx_zbx, &privkey).unwrap();
        let signed_zusd = TxSigner::sign_transaction(tx_zusd, &privkey).unwrap();
        // Different gas tokens must produce different signing hashes and different tx hashes.
        assert_ne!(signed_zbx.hash, signed_zusd.hash,
            "gas_token must be covered by the signature — changing it must change the tx hash");
    }
}
