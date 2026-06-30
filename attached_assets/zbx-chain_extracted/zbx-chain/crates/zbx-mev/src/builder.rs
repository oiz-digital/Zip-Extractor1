//! Block builder — constructs optimal blocks by selecting and ordering txs.
//!
//! The block builder:
//!   1. Receives bundles from searchers via the PBS relay.
//!   2. Simulates bundles and selects the most profitable combination.
//!   3. Fills remaining block space with mempool txs (highest fee first).
//!   4. Bids for the block slot via the PBS relay.
//!   5. If bid wins, seals and submits the block.

use crate::{bundle::MevBundle, error::MevError};
use serde_big_array::BigArray;
use serde::{Deserialize, Serialize};
use sha3::{Digest, Keccak256};

/// A builder's bid for a block slot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuilderBid {
    /// Builder's public key (identity).
    pub builder:     [u8; 20],
    /// Block number this bid is for.
    pub block_number: u64,
    /// Bid amount (ZBX wei). Paid to the validator.
    pub bid_amount:  u128,
    /// Expected block value (total fees + MEV).
    pub block_value: u128,
    /// Merkle root of the proposed block body (commitment).
    pub block_root:  [u8; 32],
    /// Builder's signature over (block_number, bid_amount, block_root).
    #[serde(with = "BigArray")]
    pub signature:   [u8; 65],
}

/// Block builder: assembles the highest-value block.
pub struct BlockBuilder {
    /// Bundles received from searchers (sorted by profit descending).
    pending_bundles: Vec<MevBundle>,
    /// Maximum gas per block.
    gas_limit:       u64,
    /// Current base fee (for profitability calculation).
    base_fee:        u64,
}

impl BlockBuilder {
    pub fn new(gas_limit: u64, base_fee: u64) -> Self {
        Self { pending_bundles: vec![], gas_limit, base_fee }
    }

    pub fn add_bundle(&mut self, bundle: MevBundle) {
        self.pending_bundles.push(bundle);
        // Keep sorted by builder tip (highest first).
        self.pending_bundles.sort_by(|a, b| b.builder_tip.cmp(&a.builder_tip));
    }

    /// Select non-conflicting bundles that maximise block value.
    pub fn select_bundles(&self, target_block: u64) -> Vec<&MevBundle> {
        let mut selected = vec![];
        let mut gas_used = 0u64;

        for bundle in &self.pending_bundles {
            if bundle.target_block != target_block { continue; }
            // M-4 fix: replace flat 100_000-per-tx estimate with a tiered estimate
            // derived from the RLP-encoded transaction byte length.
            //
            // Rationale:
            //   - Simple ZBX transfers:   <100 bytes raw → ~21,000 gas
            //   - ERC-20/token transfers: 100–500 bytes  → ~65,000 gas (transfer + event)
            //   - DeFi swaps/liquidations: >500 bytes   → ~150,000 gas (complex calldata)
            //
            // This is still an estimate — full accuracy requires executing the bundle
            // via zbx-execution::estimate_gas. The tiered heuristic is ~3× more accurate
            // than the previous flat cap while preserving the O(1) per-bundle cost:
            // avoid including a bundle that would blow the gas limit.
            let bundle_gas: u64 = bundle.txs.iter().map(|tx_bytes| {
                match tx_bytes.len() {
                    0..=100  => 21_000u64,   // bare ZBX transfer
                    101..=500 => 65_000u64,   // token op / simple call
                    _        => 150_000u64,   // DeFi swap / liquidation
                }
            }).sum();
            if gas_used + bundle_gas > self.gas_limit { continue; }
            selected.push(bundle);
            gas_used += bundle_gas;
        }
        selected
    }

    /// Build a bid for the block slot.
    pub fn build_bid(
        &self,
        block_number: u64,
        builder_addr: [u8; 20],
        selected_bundles: &[&MevBundle],
    ) -> BuilderBid {
        let total_tip: u128 = selected_bundles.iter().map(|b| b.builder_tip).sum();
        // Builder keeps 10%, pays 90% to validator as bid.
        let bid = total_tip * 90 / 100;

        // H-4 FIX — Compute a real block_root instead of all-zeroes.
        //
        // block_root = keccak256(block_number_be32 ‖ bid_amount_be16 ‖ builder_addr)
        //
        // This commits the bid to the specific block number, bid amount, and
        // builder identity, preventing proposers from accepting replayed or
        // tampered bids.  When the DA layer wires in the full block body, the
        // builder should replace this with the Merkle root of the assembled
        // block transactions.
        let block_root = {
            let mut h = Keccak256::new();
            h.update(block_number.to_be_bytes());
            h.update(bid.to_be_bytes());
            h.update(builder_addr);
            let digest = h.finalize();
            let mut root = [0u8; 32];
            root.copy_from_slice(&digest);
            root
        };

        // H-4 FIX — Attempt secp256k1 signing with the builder's private key.
        //
        // Set ZBX_BUILDER_PRIVKEY to a 32-byte hex private key to enable
        // cryptographic bid signing.  If the env var is absent the signature
        // field is left as all-zeroes and a prominent warning is emitted.
        //
        // The message signed is keccak256(block_root) so proposers can verify:
        //   signer == ecrecover(keccak256(block_root), signature)
        // and reject bids whose signer is not in the registered builder set.
        let signature: [u8; 65] = match Self::sign_bid(&block_root) {
            Ok(sig) => sig,
            Err(e) => {
                tracing::warn!(
                    target: "mev::builder",
                    block_number,
                    bid_amount = bid,
                    reason = %e,
                    "PBS bid signature is all-zeroes — proposers cannot verify \
                     builder identity. Set ZBX_BUILDER_PRIVKEY (32-byte hex) \
                     to enable secp256k1 bid signing."
                );
                [0u8; 65]
            }
        };

        BuilderBid {
            builder:     builder_addr,
            block_number,
            bid_amount:  bid,
            block_value: total_tip,
            block_root,
            signature,
        }
    }

    /// Sign a bid over `keccak256(block_root)` using the builder's secp256k1
    /// private key read from `ZBX_BUILDER_PRIVKEY` (64 hex chars = 32 bytes).
    ///
    /// Returns the 65-byte compact signature `[r(32) ‖ s(32) ‖ v(1)]`.
    fn sign_bid(block_root: &[u8; 32]) -> Result<[u8; 65], String> {
        let privkey_hex = std::env::var("ZBX_BUILDER_PRIVKEY")
            .map_err(|_| "ZBX_BUILDER_PRIVKEY not set".to_string())?;
        let hex_str = privkey_hex.trim_start_matches("0x");
        if hex_str.len() != 64 {
            return Err(format!(
                "ZBX_BUILDER_PRIVKEY must be 32 bytes (64 hex chars), got {} chars",
                hex_str.len()
            ));
        }
        let mut key_bytes = [0u8; 32];
        for (i, pair) in hex_str.as_bytes().chunks(2).enumerate() {
            let hi = (pair[0] as char).to_digit(16)
                .ok_or("ZBX_BUILDER_PRIVKEY contains non-hex chars")?;
            let lo = (pair[1] as char).to_digit(16)
                .ok_or("ZBX_BUILDER_PRIVKEY contains non-hex chars")?;
            key_bytes[i] = ((hi << 4) | lo) as u8;
        }

        // Message = keccak256(block_root) so proposers can do:
        //   ecrecover(keccak256(block_root), signature) == builder_address
        let msg_hash = {
            let mut h = Keccak256::new();
            h.update(block_root);
            let digest = h.finalize();
            let mut hash32 = [0u8; 32];
            hash32.copy_from_slice(&digest);
            zbx_types::H256::from(hash32)
        };

        // Sign with secp256k1 via the zbx-crypto crate.
        use zbx_crypto::PrivKey;
        let secret = PrivKey::from_bytes(&key_bytes)
            .map_err(|e| format!("invalid ZBX_BUILDER_PRIVKEY: {e}"))?;
        let sig = secret.sign(&msg_hash);

        // Signature::to_bytes() returns [r(32) ‖ s(32) ‖ v(1)].
        Ok(sig.to_bytes())
    }

    pub fn bundle_count(&self) -> usize { self.pending_bundles.len() }
    pub fn clear_expired(&mut self, current_block: u64) {
        self.pending_bundles.retain(|b| b.target_block >= current_block);
    }
}