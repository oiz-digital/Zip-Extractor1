//! Raw transaction decoder for `eth_sendRawTransaction`.
//!
//! Accepts the standard Ethereum EIP-2718 encodings:
//!
//!   - **Legacy (Type 0)**: `RLP([nonce, gasPrice, gasLimit, to, value, data, v, r, s])`
//!   - **EIP-2930 (Type 1)**: `0x01 || RLP([chainId, nonce, gasPrice, gasLimit, to, value,
//!                            data, accessList, signatureYParity, signatureR, signatureS])`
//!   - **EIP-1559 (Type 2)**: `0x02 || RLP([chainId, nonce, maxPriorityFeePerGas,
//!                            maxFeePerGas, gasLimit, to, value, data, accessList,
//!                            signatureYParity, signatureR, signatureS])`
//!
//! After decoding, `SignedTransaction::compute_hash()` produces the canonical
//! EIP-2718 wire hash (keccak256 of the full signed encoding), which matches
//! the hash MetaMask / ethers.js / web3.py report for the same transaction.
//!
//! Sender recovery uses the real secp256k1 ECDSA recoverer in `zbx_crypto`,
//! enforcing low-S normalisation. The recovered address is stored in
//! `SignedTransaction::from` and re-verified at mempool admission.

use crate::error::RpcError;
use zbx_rlp::Rlp;
use zbx_types::{
    address::Address,
    transaction::{Signature, SignedTransaction, Transaction, TxType},
    H256, U256,
};

/// Decode an Ethereum raw transaction (legacy or EIP-2718 typed envelope).
///
/// Returns the constructed `SignedTransaction` plus its canonical EIP-2718
/// transaction hash (keccak256 of the full signed encoding). This hash matches
/// what MetaMask / ethers.js / web3.py report and what block explorers index.
pub fn decode_raw_tx(raw: &[u8]) -> Result<(SignedTransaction, H256), RpcError> {
    if raw.is_empty() {
        return Err(RpcError::InvalidParams("empty rawTransaction".into()));
    }

    let first = raw[0];

    // EIP-2718 typed envelope when first byte is < 0x80 (RLP list prefix is >= 0xc0).
    let (tx, sig) = if first < 0x80 {
        match first {
            0x01 => decode_eip2930(&raw[1..])?,
            0x02 => decode_eip1559(&raw[1..])?,
            other => {
                return Err(RpcError::InvalidParams(format!(
                    "unsupported tx envelope type 0x{:02x}",
                    other
                )))
            }
        }
    } else {
        decode_legacy(raw)?
    };

    // Recover sender via EIP-2718-compliant signing hash + real secp256k1 ECDSA recovery.
    // Low-S normalisation is enforced inside zbx_crypto::recover_signer.
    let signing_hash = tx.signing_hash();
    let from = zbx_crypto::recover_signer(&signing_hash, &to_crypto_sig(&sig))
        .map_err(|e| RpcError::Internal(format!("recover_signer: {e}")))?;

    // Compute canonical EIP-2718 tx hash and store it. This is the hash that
    // eth_sendRawTransaction returns and eth_getTransactionByHash accepts.
    let mut signed = SignedTransaction {
        tx,
        sig,
        from,
        hash: H256::default(),
    };
    signed.hash = signed.compute_hash();
    let tx_hash = signed.hash;
    Ok((signed, tx_hash))
}

fn to_crypto_sig(sig: &Signature) -> zbx_crypto::Signature {
    zbx_crypto::Signature {
        v: sig.v,
        r: sig.r,
        s: sig.s,
    }
}

// ---------------------------------------------------------------------------
// Per-type decoders
// ---------------------------------------------------------------------------

fn decode_legacy(bytes: &[u8]) -> Result<(Transaction, Signature), RpcError> {
    let rlp = Rlp::new(bytes);
    let count = rlp
        .item_count()
        .map_err(|e| RpcError::InvalidParams(format!("legacy tx rlp: {e}")))?;
    if count != 9 {
        return Err(RpcError::InvalidParams(format!(
            "legacy tx must have 9 fields, got {count}"
        )));
    }
    let nonce = read_u64(&rlp, 0, "nonce")?;
    let gas_price = read_u64(&rlp, 1, "gasPrice")?;
    let gas_limit = read_u64(&rlp, 2, "gasLimit")?;
    let to = read_optional_address(&rlp, 3, "to")?;
    let value = read_u256(&rlp, 4, "value")?;
    let data = read_bytes(&rlp, 5, "data")?;
    let v = read_u64(&rlp, 6, "v")?;
    let r = read_h256(&rlp, 7, "r")?;
    let s = read_h256(&rlp, 8, "s")?;

    // EIP-155: chain_id = (v - 35) / 2 if v >= 35, else 0 (pre-Spurious-Dragon).
    let chain_id = if v >= 35 { (v - 35) / 2 } else { 0 };
    // y_parity = v - (chain_id*2 + 35), normalised into {0, 1}.
    let y_parity: u8 = if v >= 35 {
        ((v - 35) % 2) as u8
    } else if v == 27 || v == 28 {
        (v - 27) as u8
    } else {
        return Err(RpcError::InvalidParams(format!("invalid legacy v: {v}")));
    };

    let tx = Transaction {
        tx_type: TxType::Legacy,
        nonce,
        max_priority_fee_per_gas: gas_price,
        max_fee_per_gas: gas_price,
        gas_limit,
        to,
        value,
        data,
        access_list: Vec::new(),
        chain_id,
    };
    Ok((tx, Signature { v: y_parity, r, s }))
}

fn decode_eip2930(bytes: &[u8]) -> Result<(Transaction, Signature), RpcError> {
    let rlp = Rlp::new(bytes);
    let count = rlp
        .item_count()
        .map_err(|e| RpcError::InvalidParams(format!("eip2930 tx rlp: {e}")))?;
    if count != 11 {
        return Err(RpcError::InvalidParams(format!(
            "eip2930 tx must have 11 fields, got {count}"
        )));
    }
    let chain_id = read_u64(&rlp, 0, "chainId")?;
    let nonce = read_u64(&rlp, 1, "nonce")?;
    let gas_price = read_u64(&rlp, 2, "gasPrice")?;
    let gas_limit = read_u64(&rlp, 3, "gasLimit")?;
    let to = read_optional_address(&rlp, 4, "to")?;
    let value = read_u256(&rlp, 5, "value")?;
    let data = read_bytes(&rlp, 6, "data")?;
    let access_list = read_access_list(&rlp, 7)?;
    let y_parity = read_u64(&rlp, 8, "yParity")? as u8;
    let r = read_h256(&rlp, 9, "r")?;
    let s = read_h256(&rlp, 10, "s")?;

    let tx = Transaction {
        tx_type: TxType::AccessList,
        nonce,
        max_priority_fee_per_gas: gas_price,
        max_fee_per_gas: gas_price,
        gas_limit,
        to,
        value,
        data,
        access_list,
        chain_id,
    };
    Ok((tx, Signature { v: y_parity, r, s }))
}

fn decode_eip1559(bytes: &[u8]) -> Result<(Transaction, Signature), RpcError> {
    let rlp = Rlp::new(bytes);
    let count = rlp
        .item_count()
        .map_err(|e| RpcError::InvalidParams(format!("eip1559 tx rlp: {e}")))?;
    if count != 12 {
        return Err(RpcError::InvalidParams(format!(
            "eip1559 tx must have 12 fields, got {count}"
        )));
    }
    let chain_id = read_u64(&rlp, 0, "chainId")?;
    let nonce = read_u64(&rlp, 1, "nonce")?;
    let max_priority_fee_per_gas = read_u64(&rlp, 2, "maxPriorityFeePerGas")?;
    let max_fee_per_gas = read_u64(&rlp, 3, "maxFeePerGas")?;
    let gas_limit = read_u64(&rlp, 4, "gasLimit")?;
    let to = read_optional_address(&rlp, 5, "to")?;
    let value = read_u256(&rlp, 6, "value")?;
    let data = read_bytes(&rlp, 7, "data")?;
    let access_list = read_access_list(&rlp, 8)?;
    let y_parity = read_u64(&rlp, 9, "yParity")? as u8;
    let r = read_h256(&rlp, 10, "r")?;
    let s = read_h256(&rlp, 11, "s")?;

    let tx = Transaction {
        tx_type: TxType::DynamicFee,
        nonce,
        max_priority_fee_per_gas,
        max_fee_per_gas,
        gas_limit,
        to,
        value,
        data,
        access_list,
        chain_id,
    };
    Ok((tx, Signature { v: y_parity, r, s }))
}

// ---------------------------------------------------------------------------
// Field readers
// ---------------------------------------------------------------------------

fn read_bytes(rlp: &Rlp<'_>, idx: usize, label: &str) -> Result<Vec<u8>, RpcError> {
    rlp.at(idx)
        .and_then(|item| item.as_bytes().map(|b| b.to_vec()))
        .map_err(|e| RpcError::InvalidParams(format!("{label}: {e}")))
}

fn read_u64(rlp: &Rlp<'_>, idx: usize, label: &str) -> Result<u64, RpcError> {
    let bytes = read_bytes(rlp, idx, label)?;
    if bytes.len() > 8 {
        return Err(RpcError::InvalidParams(format!(
            "{label}: u64 overflow ({} bytes)",
            bytes.len()
        )));
    }
    let mut v = 0u64;
    for &b in &bytes {
        v = (v << 8) | b as u64;
    }
    Ok(v)
}

fn read_h256(rlp: &Rlp<'_>, idx: usize, label: &str) -> Result<H256, RpcError> {
    let bytes = read_bytes(rlp, idx, label)?;
    if bytes.len() > 32 {
        return Err(RpcError::InvalidParams(format!(
            "{label}: too long ({} bytes)",
            bytes.len()
        )));
    }
    let mut out = [0u8; 32];
    out[32 - bytes.len()..].copy_from_slice(&bytes);
    Ok(H256(out))
}

fn read_u256(rlp: &Rlp<'_>, idx: usize, label: &str) -> Result<U256, RpcError> {
    let h = read_h256(rlp, idx, label)?;
    Ok(U256::from_big_endian(h.as_bytes()))
}

fn read_optional_address(
    rlp: &Rlp<'_>,
    idx: usize,
    label: &str,
) -> Result<Option<Address>, RpcError> {
    let bytes = read_bytes(rlp, idx, label)?;
    if bytes.is_empty() {
        return Ok(None); // contract creation
    }
    if bytes.len() != 20 {
        return Err(RpcError::InvalidParams(format!(
            "{label}: address must be 20 bytes, got {}",
            bytes.len()
        )));
    }
    let mut a = [0u8; 20];
    a.copy_from_slice(&bytes);
    Ok(Some(Address(a)))
}

fn read_access_list(
    rlp: &Rlp<'_>,
    idx: usize,
) -> Result<Vec<(Address, Vec<H256>)>, RpcError> {
    let outer = rlp
        .at(idx)
        .map_err(|e| RpcError::InvalidParams(format!("accessList: {e}")))?;
    let n = outer
        .item_count()
        .map_err(|e| RpcError::InvalidParams(format!("accessList items: {e}")))?;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let entry = outer
            .at(i)
            .map_err(|e| RpcError::InvalidParams(format!("accessList[{i}]: {e}")))?;
        let addr = read_optional_address(&entry, 0, "accessList.addr")?
            .ok_or_else(|| RpcError::InvalidParams("accessList.addr empty".into()))?;
        let slots_outer = entry
            .at(1)
            .map_err(|e| RpcError::InvalidParams(format!("accessList[{i}].slots: {e}")))?;
        let m = slots_outer
            .item_count()
            .map_err(|e| RpcError::InvalidParams(format!("accessList[{i}].slots items: {e}")))?;
        let mut slots = Vec::with_capacity(m);
        for j in 0..m {
            slots.push(read_h256(&slots_outer, j, "slot")?);
        }
        out.push((addr, slots));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Empty input must reject.
    #[test]
    fn rejects_empty() {
        assert!(decode_raw_tx(&[]).is_err());
    }

    /// Unknown envelope byte must reject.
    #[test]
    fn rejects_unknown_envelope() {
        assert!(decode_raw_tx(&[0x05, 0xc0]).is_err());
    }
}
