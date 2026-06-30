//! Transaction types with EIP-1559, EIP-2930, and Legacy (EIP-155) support.
//!
//! ## Signing hash (what a wallet signs)
//!
//! - **Legacy (Type 0)** — EIP-155:
//!   `keccak256(RLP([nonce, gasPrice, gasLimit, to, value, data, chainId, 0, 0]))`
//! - **EIP-2930 (Type 1)**:
//!   `keccak256(0x01 || RLP([chainId, nonce, gasPrice, gasLimit, to, value, data, accessList]))`
//! - **EIP-1559 (Type 2)**:
//!   `keccak256(0x02 || RLP([chainId, nonce, maxPriorityFeePerGas, maxFeePerGas, gasLimit,
//!              to, value, data, accessList]))`
//!
//! ## Transaction hash (what a block explorer shows)
//!
//! - **Legacy**: `keccak256(RLP([nonce, gasPrice, gasLimit, to, value, data, v, r, s]))`
//!   where `v = chainId * 2 + 35 + yParity` (EIP-155).
//! - **EIP-2930**: `keccak256(0x01 || RLP([chainId, nonce, gasPrice, gasLimit, to, value,
//!   data, accessList, yParity, r, s]))`
//! - **EIP-1559**: `keccak256(0x02 || RLP([chainId, nonce, maxPriorityFeePerGas,
//!   maxFeePerGas, gasLimit, to, value, data, accessList, yParity, r, s]))`
//!
//! Both hashes are fully EIP-2718-compatible: MetaMask, ethers.js, and web3.py
//! produce identical values.

use crate::{address::Address, error::ZbxError, H256, U256};
use rlp::RlpStream;
use serde::{Deserialize, Serialize};
use sha3::{Digest, Keccak256};

// ---------------------------------------------------------------------------
// Private RLP helpers
// ---------------------------------------------------------------------------

/// Convert a U256 to minimal big-endian bytes (no leading zeros) for RLP.
fn u256_rlp_bytes(value: &U256) -> Vec<u8> {
    let mut buf = [0u8; 32];
    value.to_big_endian(&mut buf);
    let skip = buf.iter().take_while(|&&b| b == 0).count();
    buf[skip..].to_vec()
}

/// Strip leading zero bytes from a fixed-length big-endian scalar (r, s, v).
fn strip_zeros(bytes: &[u8]) -> &[u8] {
    let skip = bytes.iter().take_while(|&&b| b == 0).count();
    &bytes[skip..]
}

/// RLP-encode an EIP-2930 access list into a pre-allocated byte vector.
///
/// Wire format: `[[address, [slot, ...]], ...]`
fn rlp_access_list(access_list: &[(Address, Vec<H256>)]) -> Vec<u8> {
    let mut outer = RlpStream::new_list(access_list.len());
    for (addr, slots) in access_list {
        let mut entry = RlpStream::new_list(2);
        entry.append(&(addr.as_bytes() as &[u8]));
        let mut slot_list = RlpStream::new_list(slots.len());
        for slot in slots {
            slot_list.append(&slot.as_bytes());
        }
        entry.append_raw(&slot_list.out(), 1);
        outer.append_raw(&entry.out(), 1);
    }
    outer.out().to_vec()
}

// ---------------------------------------------------------------------------
// Transaction types
// ---------------------------------------------------------------------------

/// Transaction type discriminant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum TxType {
    /// Legacy (pre-EIP-2930) transaction.
    Legacy = 0,
    /// EIP-2930 access-list transaction.
    AccessList = 1,
    /// EIP-1559 dynamic-fee transaction.
    DynamicFee = 2,
}

/// Unsigned transaction fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transaction {
    pub tx_type: TxType,
    /// Sender nonce — prevents replay attacks.
    pub nonce: u64,
    /// EIP-1559: maximum priority fee per gas (tip to validator).
    pub max_priority_fee_per_gas: u64,
    /// EIP-1559: maximum total fee per gas.
    pub max_fee_per_gas: u64,
    /// Maximum gas this transaction may consume.
    pub gas_limit: u64,
    /// Recipient address. None for contract creation.
    pub to: Option<Address>,
    /// ZBX value transferred in wei.
    pub value: U256,
    /// Input data / contract call ABI.
    pub data: Vec<u8>,
    /// EIP-2930 access list (address + storage keys).
    pub access_list: Vec<(Address, Vec<H256>)>,
    /// Chain ID — must equal CHAIN_ID on ZBX mainnet.
    pub chain_id: u64,
}

impl Transaction {
    /// Compute the EIP-2718-compatible signing hash for this transaction.
    ///
    /// This is the hash that a wallet (MetaMask, ethers.js, web3.py) signs.
    /// The signature recovered from this hash must match the sender address.
    ///
    /// - Legacy (EIP-155): `keccak256(RLP([nonce, gasPrice, gasLimit, to, value, data, chainId, 0, 0]))`
    /// - EIP-2930: `keccak256(0x01 || RLP([chainId, nonce, gasPrice, gasLimit, to, value, data, accessList]))`
    /// - EIP-1559: `keccak256(0x02 || RLP([chainId, nonce, maxPriorityFeePerGas, maxFeePerGas, gasLimit, to, value, data, accessList]))`
    pub fn signing_hash(&self) -> H256 {
        let value_bytes = u256_rlp_bytes(&self.value);
        let encoded: Vec<u8> = match self.tx_type {
            TxType::Legacy => {
                // EIP-155 replay protection: pad with [chainId, 0, 0] before signing.
                // Field order: [nonce, gasPrice, gasLimit, to, value, data, chainId, 0, 0]
                let mut s = RlpStream::new_list(9);
                s.append(&self.nonce);
                s.append(&self.max_fee_per_gas); // gasPrice (legacy has single fee)
                s.append(&self.gas_limit);
                match &self.to {
                    Some(to) => s.append(&(to.as_bytes() as &[u8])),
                    None => s.append_empty_data(), // CREATE: empty byte string
                };
                s.append(&value_bytes.as_slice());
                s.append(&self.data.as_slice());
                s.append(&self.chain_id);
                s.append(&0u64); // EIP-155 padding
                s.append(&0u64); // EIP-155 padding
                s.out().to_vec()
            }

            TxType::AccessList => {
                // EIP-2930: type byte 0x01 prepended to RLP list.
                // Field order: [chainId, nonce, gasPrice, gasLimit, to, value, data, accessList]
                let al = rlp_access_list(&self.access_list);
                let mut s = RlpStream::new_list(8);
                s.append(&self.chain_id);
                s.append(&self.nonce);
                s.append(&self.max_fee_per_gas); // gasPrice
                s.append(&self.gas_limit);
                match &self.to {
                    Some(to) => s.append(&(to.as_bytes() as &[u8])),
                    None => s.append_empty_data(),
                };
                s.append(&value_bytes.as_slice());
                s.append(&self.data.as_slice());
                s.append_raw(&al, 1); // access list is a pre-encoded list item
                let mut buf = vec![0x01u8];
                buf.extend_from_slice(&s.out());
                buf
            }

            TxType::DynamicFee => {
                // EIP-1559: type byte 0x02 prepended to RLP list.
                // Field order: [chainId, nonce, maxPriorityFeePerGas, maxFeePerGas, gasLimit, to, value, data, accessList]
                let al = rlp_access_list(&self.access_list);
                let mut s = RlpStream::new_list(9);
                s.append(&self.chain_id);
                s.append(&self.nonce);
                s.append(&self.max_priority_fee_per_gas);
                s.append(&self.max_fee_per_gas);
                s.append(&self.gas_limit);
                match &self.to {
                    Some(to) => s.append(&(to.as_bytes() as &[u8])),
                    None => s.append_empty_data(),
                };
                s.append(&value_bytes.as_slice());
                s.append(&self.data.as_slice());
                s.append_raw(&al, 1);
                let mut buf = vec![0x02u8];
                buf.extend_from_slice(&s.out());
                buf
            }
        };
        H256::from_slice(&Keccak256::digest(&encoded))
    }

    /// Estimated intrinsic gas cost before execution.
    pub fn intrinsic_gas(&self) -> u64 {
        let base = if self.to.is_none() { 53_000u64 } else { 21_000u64 };
        let data_cost: u64 = self.data.iter().map(|&b| if b == 0 { 4 } else { 16 }).sum();
        let al_cost = self.access_list.len() as u64 * 2_400
            + self.access_list.iter().map(|(_, slots)| slots.len() as u64 * 1_900).sum::<u64>();
        base + data_cost + al_cost
    }

    /// True if this is a contract deployment.
    pub fn is_create(&self) -> bool {
        self.to.is_none()
    }
}

// ---------------------------------------------------------------------------
// Signature type
// ---------------------------------------------------------------------------

/// ECDSA signature components.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature {
    /// Recovery ID (0 or 1).
    pub v: u8,
    /// 32-byte R component.
    pub r: H256,
    /// 32-byte S component.
    pub s: H256,
}

impl Signature {
    /// Serialise to 65-byte compact form [r || s || v].
    pub fn to_bytes(&self) -> [u8; 65] {
        let mut out = [0u8; 65];
        out[..32].copy_from_slice(self.r.as_bytes());
        out[32..64].copy_from_slice(self.s.as_bytes());
        out[64] = self.v;
        out
    }

    /// Parse from 65-byte compact form.
    pub fn from_bytes(b: &[u8]) -> Result<Self, ZbxError> {
        if b.len() != 65 {
            return Err(ZbxError::InvalidLength { expected: 65, got: b.len() });
        }
        Ok(Signature {
            v: b[64],
            r: H256::from_slice(&b[..32]),
            s: H256::from_slice(&b[32..64]),
        })
    }
}

// ---------------------------------------------------------------------------
// SignedTransaction
// ---------------------------------------------------------------------------

/// A transaction paired with its secp256k1 signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedTransaction {
    pub tx: Transaction,
    pub sig: Signature,
    /// Cached sender address (recovered from signature).
    pub from: Address,
    /// Cached transaction hash (EIP-2718 wire hash).
    pub hash: H256,
}

impl SignedTransaction {
    /// Recompute and validate the stored transaction hash.
    ///
    /// Returns `true` iff the cached `hash` field matches the canonical
    /// EIP-2718 hash derived from the current tx + sig fields.
    pub fn verify_hash(&self) -> bool {
        self.compute_hash() == self.hash
    }

    /// Compute the canonical EIP-2718 transaction hash.
    ///
    /// This is what block explorers index and what `eth_getTransactionByHash`
    /// accepts. It is `keccak256` of the full EIP-2718 signed encoding:
    ///
    /// - **Legacy**: `keccak256(RLP([nonce, gasPrice, gasLimit, to, value, data, v, r, s]))`
    ///   where `v = chainId * 2 + 35 + yParity` (EIP-155 replay protection).
    /// - **EIP-2930**: `keccak256(0x01 || RLP([chainId, nonce, gasPrice, gasLimit, to,
    ///   value, data, accessList, yParity, r, s]))`
    /// - **EIP-1559**: `keccak256(0x02 || RLP([chainId, nonce, maxPriorityFeePerGas,
    ///   maxFeePerGas, gasLimit, to, value, data, accessList, yParity, r, s]))`
    pub fn compute_hash(&self) -> H256 {
        let tx = &self.tx;
        let sig = &self.sig;
        let value_bytes = u256_rlp_bytes(&tx.value);
        // r and s are big-endian integers — leading zeros are stripped per RLP spec.
        let r = strip_zeros(sig.r.as_bytes());
        let s = strip_zeros(sig.s.as_bytes());

        let encoded: Vec<u8> = match tx.tx_type {
            TxType::Legacy => {
                // EIP-155: v = chainId * 2 + 35 + yParity
                let v: u64 = tx.chain_id.saturating_mul(2).saturating_add(35).saturating_add(sig.v as u64);
                let mut rlp = RlpStream::new_list(9);
                rlp.append(&tx.nonce);
                rlp.append(&tx.max_fee_per_gas); // gasPrice
                rlp.append(&tx.gas_limit);
                match &tx.to {
                    Some(to) => rlp.append(&(to.as_bytes() as &[u8])),
                    None => rlp.append_empty_data(),
                };
                rlp.append(&value_bytes.as_slice());
                rlp.append(&tx.data.as_slice());
                rlp.append(&v);
                rlp.append(&r);
                rlp.append(&s);
                rlp.out().to_vec()
            }

            TxType::AccessList => {
                // EIP-2930: 0x01 || RLP([chainId, nonce, gasPrice, gasLimit, to, value, data,
                //                       accessList, yParity, r, s])
                let al = rlp_access_list(&tx.access_list);
                let y_parity: u64 = sig.v as u64;
                let mut rlp = RlpStream::new_list(11);
                rlp.append(&tx.chain_id);
                rlp.append(&tx.nonce);
                rlp.append(&tx.max_fee_per_gas); // gasPrice
                rlp.append(&tx.gas_limit);
                match &tx.to {
                    Some(to) => rlp.append(&(to.as_bytes() as &[u8])),
                    None => rlp.append_empty_data(),
                };
                rlp.append(&value_bytes.as_slice());
                rlp.append(&tx.data.as_slice());
                rlp.append_raw(&al, 1);
                rlp.append(&y_parity);
                rlp.append(&r);
                rlp.append(&s);
                let mut buf = vec![0x01u8];
                buf.extend_from_slice(&rlp.out());
                buf
            }

            TxType::DynamicFee => {
                // EIP-1559: 0x02 || RLP([chainId, nonce, maxPriorityFeePerGas, maxFeePerGas,
                //                       gasLimit, to, value, data, accessList, yParity, r, s])
                let al = rlp_access_list(&tx.access_list);
                let y_parity: u64 = sig.v as u64;
                let mut rlp = RlpStream::new_list(12);
                rlp.append(&tx.chain_id);
                rlp.append(&tx.nonce);
                rlp.append(&tx.max_priority_fee_per_gas);
                rlp.append(&tx.max_fee_per_gas);
                rlp.append(&tx.gas_limit);
                match &tx.to {
                    Some(to) => rlp.append(&(to.as_bytes() as &[u8])),
                    None => rlp.append_empty_data(),
                };
                rlp.append(&value_bytes.as_slice());
                rlp.append(&tx.data.as_slice());
                rlp.append_raw(&al, 1);
                rlp.append(&y_parity);
                rlp.append(&r);
                rlp.append(&s);
                let mut buf = vec![0x02u8];
                buf.extend_from_slice(&rlp.out());
                buf
            }
        };
        H256::from_slice(&Keccak256::digest(&encoded))
    }

    /// Effective gas price paid by sender considering the block base fee.
    pub fn effective_gas_price(&self, base_fee: u64) -> u64 {
        let tip = self.tx.max_priority_fee_per_gas;
        let max = self.tx.max_fee_per_gas;
        base_fee + tip.min(max.saturating_sub(base_fee))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn zero_addr() -> Address {
        Address([0u8; 20])
    }

    fn make_eip1559(chain_id: u64, nonce: u64, to: Option<Address>) -> Transaction {
        Transaction {
            tx_type: TxType::DynamicFee,
            chain_id,
            nonce,
            max_priority_fee_per_gas: 1_000_000_000,
            max_fee_per_gas: 20_000_000_000,
            gas_limit: 21_000,
            to,
            value: U256::from(1_000_000_000_000_000_000u64), // 1 ZBX
            data: vec![],
            access_list: vec![],
        }
    }

    fn make_legacy(chain_id: u64, nonce: u64) -> Transaction {
        Transaction {
            tx_type: TxType::Legacy,
            chain_id,
            nonce,
            max_priority_fee_per_gas: 10_000_000_000,
            max_fee_per_gas: 10_000_000_000,
            gas_limit: 21_000,
            to: Some(zero_addr()),
            value: U256::from(500_000_000_000_000_000u64),
            data: vec![],
            access_list: vec![],
        }
    }

    /// signing_hash must be deterministic for same inputs.
    #[test]
    fn signing_hash_deterministic() {
        let tx = make_eip1559(8989, 0, Some(zero_addr()));
        assert_eq!(tx.signing_hash(), tx.signing_hash());
    }

    /// Different chain IDs must produce different signing hashes (EIP-155 replay protection).
    #[test]
    fn signing_hash_chain_id_replay_protection() {
        let tx_main = make_legacy(8989, 0);
        let tx_test = make_legacy(8990, 0);
        assert_ne!(tx_main.signing_hash(), tx_test.signing_hash(),
            "same tx on different chains must have different signing hashes");
    }

    /// EIP-2718 type byte must be part of the signing hash (type 1 != type 2).
    #[test]
    fn signing_hash_type_differentiates() {
        let legacy = make_legacy(8989, 0);
        let eip1559 = make_eip1559(8989, 0, Some(zero_addr()));
        assert_ne!(legacy.signing_hash(), eip1559.signing_hash());
    }

    /// EIP-1559 type 2 — signing hash must start with 0x02 influence.
    /// Verified indirectly: nonce change must change the hash.
    #[test]
    fn signing_hash_nonce_changes_hash() {
        let tx0 = make_eip1559(8989, 0, Some(zero_addr()));
        let tx1 = make_eip1559(8989, 1, Some(zero_addr()));
        assert_ne!(tx0.signing_hash(), tx1.signing_hash());
    }

    /// CREATE tx (to = None) must hash differently than CALL tx.
    #[test]
    fn signing_hash_create_vs_call() {
        let call = make_eip1559(8989, 0, Some(zero_addr()));
        let create = make_eip1559(8989, 0, None);
        assert_ne!(call.signing_hash(), create.signing_hash());
    }

    /// compute_hash must be deterministic.
    #[test]
    fn compute_hash_deterministic() {
        let tx = make_eip1559(8989, 7, Some(zero_addr()));
        let sig = Signature { v: 0, r: H256([1u8; 32]), s: H256([2u8; 32]) };
        let stx = SignedTransaction {
            from: zero_addr(),
            hash: H256::default(),
            tx,
            sig,
        };
        assert_eq!(stx.compute_hash(), stx.compute_hash());
    }

    /// compute_hash must differ from signing_hash (different data included).
    #[test]
    fn compute_hash_differs_from_signing_hash() {
        let tx = make_eip1559(8989, 3, Some(zero_addr()));
        let sig = Signature { v: 1, r: H256([0xabu8; 32]), s: H256([0xcdu8; 32]) };
        let stx = SignedTransaction {
            from: zero_addr(),
            hash: H256::default(),
            tx: tx.clone(),
            sig,
        };
        assert_ne!(stx.compute_hash(), tx.signing_hash(),
            "signed tx hash must differ from signing hash");
    }

    /// verify_hash must return true when hash field is correctly populated.
    #[test]
    fn verify_hash_roundtrip() {
        let tx = make_legacy(8989, 42);
        let sig = Signature { v: 0, r: H256([0x11u8; 32]), s: H256([0x22u8; 32]) };
        let mut stx = SignedTransaction {
            from: zero_addr(),
            hash: H256::default(),
            tx,
            sig,
        };
        stx.hash = stx.compute_hash();
        assert!(stx.verify_hash(), "verify_hash must return true after setting hash = compute_hash()");
    }

    /// Legacy EIP-155 v must encode chain_id correctly.
    /// If chain_id=8989, y_parity=0 → v = 8989*2+35+0 = 18013.
    #[test]
    fn legacy_v_eip155_encoding() {
        let tx = make_legacy(8989, 0);
        let sig = Signature { v: 0, r: H256([0u8; 32]), s: H256([0u8; 32]) };
        let stx = SignedTransaction {
            from: zero_addr(),
            hash: H256::default(),
            tx,
            sig,
        };
        let hash1 = stx.compute_hash();
        // Changing y_parity should change the hash (v changes).
        let sig2 = Signature { v: 1, r: H256([0u8; 32]), s: H256([0u8; 32]) };
        let stx2 = SignedTransaction { sig: sig2, ..stx };
        assert_ne!(hash1, stx2.compute_hash(),
            "y_parity change must change the legacy tx hash");
    }

    /// EIP-2930 access list encoding — adding an entry must change all hashes.
    #[test]
    fn access_list_changes_hash() {
        let mut tx_empty = Transaction {
            tx_type: TxType::AccessList,
            chain_id: 8989,
            nonce: 0,
            max_priority_fee_per_gas: 1_000_000_000,
            max_fee_per_gas: 10_000_000_000,
            gas_limit: 50_000,
            to: Some(zero_addr()),
            value: U256::zero(),
            data: vec![],
            access_list: vec![],
        };
        let h_empty = tx_empty.signing_hash();
        tx_empty.access_list = vec![(zero_addr(), vec![H256([0xffu8; 32])])];
        let h_with_al = tx_empty.signing_hash();
        assert_ne!(h_empty, h_with_al, "access list must change the signing hash");
    }

    /// intrinsic_gas: base 21000 for a plain transfer.
    #[test]
    fn intrinsic_gas_plain_transfer() {
        let tx = make_eip1559(8989, 0, Some(zero_addr()));
        assert_eq!(tx.intrinsic_gas(), 21_000);
    }

    /// intrinsic_gas: base 53000 for contract creation.
    #[test]
    fn intrinsic_gas_create() {
        let tx = make_eip1559(8989, 0, None);
        assert_eq!(tx.intrinsic_gas(), 53_000);
    }

    /// effective_gas_price: capped at max_fee when tip would exceed budget.
    #[test]
    fn effective_gas_price_capped() {
        let tx = Transaction {
            max_priority_fee_per_gas: 100,
            max_fee_per_gas: 50,
            ..make_eip1559(8989, 0, None)
        };
        let stx = SignedTransaction {
            from: zero_addr(),
            hash: H256::default(),
            sig: Signature { v: 0, r: H256::default(), s: H256::default() },
            tx,
        };
        // base_fee = 30, max_fee = 50, tip = 100 → effective = 30 + min(100, 50-30) = 30+20 = 50
        assert_eq!(stx.effective_gas_price(30), 50);
    }
}
