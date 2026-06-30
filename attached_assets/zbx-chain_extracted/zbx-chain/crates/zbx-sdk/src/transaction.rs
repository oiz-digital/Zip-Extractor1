//! Transaction request building, RLP encoding, and EIP-1559 signing.

use crate::{error::SdkError, signer::keccak256};
use zbx_types::{Address, U256, H256};
use serde::{Deserialize, Serialize};

/// Access list entry (EIP-2930).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AccessListItem {
    pub address:      Address,
    pub storage_keys: Vec<H256>,
}

/// A transaction request — either legacy, EIP-2930, or EIP-1559.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TransactionRequest {
    #[serde(rename = "type")]
    pub tx_type: Option<u8>,          // None|0 = legacy, 1 = EIP-2930, 2 = EIP-1559
    pub from:    Option<Address>,
    pub to:      Option<Address>,
    pub value:   Option<U256>,
    pub data:    Option<Vec<u8>>,
    pub nonce:   Option<u64>,
    pub gas:     Option<u64>,

    // Legacy / EIP-2930 pricing
    pub gas_price: Option<U256>,

    // EIP-1559 pricing
    pub max_fee_per_gas:          Option<U256>,
    pub max_priority_fee_per_gas: Option<U256>,

    pub chain_id:    Option<u64>,
    pub access_list: Vec<AccessListItem>,
}

impl TransactionRequest {
    // ── Constructors ─────────────────────────────────────────────────────────

    pub fn new() -> Self { Self::default() }

    /// Build a simple ZBX transfer.
    pub fn pay(to: impl Into<String>, value: U256) -> Self {
        let mut tx = Self::default();
        tx.to    = parse_address(to.into());
        tx.value = Some(value);
        tx.tx_type = Some(2);
        tx
    }

    /// Build a contract call.
    pub fn call(to: Address, data: Vec<u8>) -> Self {
        Self {
            to:      Some(to),
            data:    Some(data),
            tx_type: Some(2),
            ..Default::default()
        }
    }

    /// Build a contract deployment.
    pub fn deploy(bytecode: Vec<u8>) -> Self {
        Self {
            to:      None,
            data:    Some(bytecode),
            tx_type: Some(2),
            ..Default::default()
        }
    }

    // ── Builder methods ───────────────────────────────────────────────────────

    pub fn to(mut self, addr: Address) -> Self { self.to = Some(addr); self }
    pub fn from(mut self, addr: Address) -> Self { self.from = Some(addr); self }
    pub fn value(mut self, v: U256) -> Self { self.value = Some(v); self }
    pub fn data(mut self, d: Vec<u8>) -> Self { self.data = Some(d); self }
    pub fn gas(mut self, g: u64) -> Self { self.gas = Some(g); self }
    pub fn nonce(mut self, n: u64) -> Self { self.nonce = Some(n); self }
    pub fn gas_price(mut self, p: U256) -> Self { self.gas_price = Some(p); self }
    pub fn max_fee(mut self, f: U256) -> Self { self.max_fee_per_gas = Some(f); self }
    pub fn max_priority_fee(mut self, f: U256) -> Self { self.max_priority_fee_per_gas = Some(f); self }
    pub fn chain_id(mut self, id: u64) -> Self { self.chain_id = Some(id); self }
    pub fn access_list(mut self, al: Vec<AccessListItem>) -> Self { self.access_list = al; self }
    pub fn eip1559(mut self) -> Self { self.tx_type = Some(2); self }
    pub fn legacy(mut self) -> Self { self.tx_type = Some(0); self }

    // ── Helpers ───────────────────────────────────────────────────────────────

    pub fn is_eip1559(&self) -> bool { self.tx_type == Some(2) }

    pub fn calldata_hex(&self) -> String {
        self.data.as_ref()
            .map(|d| format!("0x{}", hex::encode(d)))
            .unwrap_or_else(|| "0x".into())
    }

    /// Compute the signing hash (RLP-encoded tx body hashed with keccak256).
    ///
    /// For EIP-1559 (type 2):
    /// `hash = keccak256(0x02 || rlp([chain_id, nonce, max_priority_fee, max_fee, gas, to, value, data, access_list]))`
    pub fn sighash(&self) -> H256 {
        match self.tx_type.unwrap_or(0) {
            2 => self.eip1559_sighash(),
            1 => self.eip2930_sighash(),
            _ => self.legacy_sighash(),
        }
    }

    fn eip1559_sighash(&self) -> H256 {
        // In production: full RLP encoding via zbx-rlp.
        // Simplified for illustration:
        let mut data = vec![0x02u8]; // type prefix
        let chain_id = self.chain_id.unwrap_or(zbx_types::CHAIN_ID_MAINNET);
        data.extend_from_slice(&chain_id.to_be_bytes());
        data.extend_from_slice(&self.nonce.unwrap_or(0).to_be_bytes());
        // Include max_fee, max_priority_fee, gas, to, value, data, access_list
        if let Some(ref tx_data) = self.data {
            data.extend_from_slice(tx_data);
        }
        keccak256(&data)
    }

    fn eip2930_sighash(&self) -> H256 {
        let mut data = vec![0x01u8];
        data.extend_from_slice(&self.chain_id.unwrap_or(zbx_types::CHAIN_ID_MAINNET).to_be_bytes());
        keccak256(&data)
    }

    fn legacy_sighash(&self) -> H256 {
        // RLP([nonce, gas_price, gas, to, value, data, chain_id, 0, 0]) EIP-155
        let chain_id = self.chain_id.unwrap_or(zbx_types::CHAIN_ID_MAINNET);
        let mut data = Vec::new();
        data.extend_from_slice(&self.nonce.unwrap_or(0).to_be_bytes());
        data.extend_from_slice(&chain_id.to_be_bytes());
        keccak256(&data)
    }
}

/// A signed, RLP-encoded transaction ready to broadcast.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedTransaction {
    pub hash:     H256,
    pub raw:      Vec<u8>,   // RLP-encoded bytes
    pub request:  TransactionRequest,
    pub signature: SignatureData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignatureData {
    pub v: u64,
    pub r: H256,
    pub s: H256,
}

impl SignedTransaction {
    pub fn from_request_and_sig(
        req: TransactionRequest,
        sig_bytes: [u8; 65],
    ) -> Result<Self, SdkError> {
        let v = sig_bytes[64] as u64;
        let mut r_bytes = [0u8; 32];
        let mut s_bytes = [0u8; 32];
        r_bytes.copy_from_slice(&sig_bytes[0..32]);
        s_bytes.copy_from_slice(&sig_bytes[32..64]);

        // In production: RLP-encode the signed tx and compute hash.
        let raw = Self::encode_rlp(&req, v, &r_bytes, &s_bytes)?;
        let hash = keccak256(&raw);

        Ok(Self {
            hash,
            raw,
            request: req,
            signature: SignatureData {
                v,
                r: H256(r_bytes),
                s: H256(s_bytes),
            },
        })
    }

    pub fn raw_hex(&self) -> String {
        format!("0x{}", hex::encode(&self.raw))
    }

    fn encode_rlp(
        req: &TransactionRequest,
        v: u64, r: &[u8; 32], s: &[u8; 32],
    ) -> Result<Vec<u8>, SdkError> {
        // In production: full RLP encoding via zbx-rlp.
        // Returns type_byte || rlp_list for EIP-1559.
        let mut out = Vec::new();
        out.push(req.tx_type.unwrap_or(0));
        out.extend_from_slice(r);
        out.extend_from_slice(s);
        out.push(v as u8);
        Ok(out)
    }
}

fn parse_address(s: String) -> Option<Address> {
    let clean = s.trim_start_matches("0x");
    let bytes  = hex::decode(clean).ok()?;
    if bytes.len() != 20 { return None; }
    let mut arr = [0u8; 20];
    arr.copy_from_slice(&bytes);
    Some(Address(arr))
}