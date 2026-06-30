//! Bundler mempool: stores and manages pending UserOperations.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use crate::error::BundlerError;

/// ERC-4337 UserOperation (v0.6 format).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserOperation {
    /// Smart wallet address (must be deployed or have initCode).
    pub sender: String,
    /// Anti-replay nonce (managed by EntryPoint nonce manager).
    pub nonce: u64,
    /// Initcode for wallet deployment (empty if already deployed).
    pub init_code: Vec<u8>,
    /// Encoded call to execute on the wallet.
    pub call_data: Vec<u8>,
    /// Gas limit for the wallet's executeOp call.
    pub call_gas_limit: u64,
    /// Gas limit for signature verification.
    pub verification_gas_limit: u64,
    /// Pre-verification gas (bundler overhead compensation).
    pub pre_verification_gas: u64,
    /// Max fee per gas (same semantics as EIP-1559).
    pub max_fee_per_gas: u128,
    /// Max priority fee per gas.
    pub max_priority_fee_per_gas: u128,
    /// Paymaster and data (empty = sender pays).
    pub paymaster_and_data: Vec<u8>,
    /// Wallet's signature over the UserOperation hash.
    pub signature: Vec<u8>,
    /// SEC-2026-05-09 Pass-15 (HIGH-R05): time-window enforcement.
    /// ERC-4337 §6 packs `validUntil` / `validAfter` into the upper
    /// bytes of the wallet-returned `validationData` uint256. Pre-fix
    /// the bundler ignored both fields entirely — a UserOp signed at
    /// time T with `validUntil=T+10` could sit in the mempool for
    /// hours and be bundled long after expiry, defeating the wallet's
    /// freshness guarantee. These are kept OUT of the canonical
    /// userOp hash (per spec — they're a simulation/bundler-side
    /// constraint, not part of the on-chain hash). `0` = no
    /// constraint (preserves backwards-compat with v0.6 callers that
    /// don't populate the field). Set by the bundler from the wallet's
    /// validationData return value during simulation.
    #[serde(default)]
    pub valid_until: u64,
    #[serde(default)]
    pub valid_after: u64,
}

impl UserOperation {
    /// SEC-2026-05-09 Pass-15 (HIGH-R05): is this UserOp currently
    /// valid against `now` (unix seconds)? `0` on either bound means
    /// "no constraint" so v0.6 callers keep working.
    pub fn is_currently_valid(&self, now: u64) -> bool {
        if self.valid_after != 0 && now < self.valid_after {
            return false;
        }
        if self.valid_until != 0 && now > self.valid_until {
            return false;
        }
        true
    }
}

impl UserOperation {
    /// Compute the ERC-4337 UserOperation hash, matching the on-chain
    /// `EntryPoint.getUserOpHash()` exactly:
    ///
    /// ```text
    /// keccak256(abi.encode(
    ///   keccak256(abi.encode(
    ///     sender, nonce, keccak256(initCode), keccak256(callData),
    ///     callGasLimit, verificationGasLimit, preVerificationGas,
    ///     maxFeePerGas, maxPriorityFeePerGas, keccak256(paymasterAndData)
    ///   )),
    ///   entryPoint,
    ///   chainId
    /// ))
    /// ```
    ///
    /// SEC-2026-05-09 Pass-13 (AA-T1-USEROP-HASH): pre-Pass-13 this was
    /// SHA-256 of a few concatenated fields ignoring callData / initCode /
    /// gas params / paymasterAndData entirely. Bundler-computed hash
    /// disagreed with on-chain `EntryPoint.getUserOpHash()` for every
    /// non-trivial UserOp → wallets signed one digest, EntryPoint
    /// validated a different one → every UserOp reverted in production.
    /// Replaced with the canonical keccak256 abi.encode form.
    pub fn hash(&self, entry_point: &str, chain_id: u64) -> [u8; 32] {
        use sha3::{Digest, Keccak256};

        // Parse entry_point hex (with or without 0x prefix) into 20 bytes.
        let ep_clean = entry_point.trim_start_matches("0x");
        let mut ep_bytes = [0u8; 20];
        if ep_clean.len() == 40 {
            if let Ok(decoded) = hex::decode(ep_clean) {
                if decoded.len() == 20 {
                    ep_bytes.copy_from_slice(&decoded);
                }
            }
        }

        // Parse sender hex into 20 bytes (zero on parse failure — this
        // matches the on-chain ABI which left-pads short addresses to 20).
        let s_clean = self.sender.trim_start_matches("0x");
        let mut sender_bytes = [0u8; 20];
        if s_clean.len() == 40 {
            if let Ok(decoded) = hex::decode(s_clean) {
                if decoded.len() == 20 {
                    sender_bytes.copy_from_slice(&decoded);
                }
            }
        }

        // ABI encoding helpers (32-byte big-endian words).
        fn pad32_addr(a: &[u8; 20]) -> [u8; 32] {
            let mut w = [0u8; 32];
            w[12..].copy_from_slice(a);
            w
        }
        fn pad32_u64(n: u64) -> [u8; 32] {
            let mut w = [0u8; 32];
            w[24..].copy_from_slice(&n.to_be_bytes());
            w
        }
        fn pad32_u128(n: u128) -> [u8; 32] {
            let mut w = [0u8; 32];
            w[16..].copy_from_slice(&n.to_be_bytes());
            w
        }
        fn keccak(data: &[u8]) -> [u8; 32] {
            let mut h = Keccak256::new();
            h.update(data);
            let mut out = [0u8; 32];
            out.copy_from_slice(&h.finalize());
            out
        }

        // Inner pack — ten 32-byte words (320 bytes total).
        let mut inner = Vec::with_capacity(320);
        inner.extend_from_slice(&pad32_addr(&sender_bytes));
        inner.extend_from_slice(&pad32_u64(self.nonce));
        inner.extend_from_slice(&keccak(&self.init_code));
        inner.extend_from_slice(&keccak(&self.call_data));
        inner.extend_from_slice(&pad32_u64(self.call_gas_limit));
        inner.extend_from_slice(&pad32_u64(self.verification_gas_limit));
        inner.extend_from_slice(&pad32_u64(self.pre_verification_gas));
        inner.extend_from_slice(&pad32_u128(self.max_fee_per_gas));
        inner.extend_from_slice(&pad32_u128(self.max_priority_fee_per_gas));
        inner.extend_from_slice(&keccak(&self.paymaster_and_data));
        let inner_hash = keccak(&inner);

        // Outer pack — userOp inner hash + entryPoint addr + chainId.
        let mut outer = Vec::with_capacity(96);
        outer.extend_from_slice(&inner_hash);
        outer.extend_from_slice(&pad32_addr(&ep_bytes));
        outer.extend_from_slice(&pad32_u64(chain_id));
        keccak(&outer)
    }

    /// Total gas limit for this UserOperation.
    pub fn total_gas(&self) -> u64 {
        self.call_gas_limit
            .saturating_add(self.verification_gas_limit)
            .saturating_add(self.pre_verification_gas)
    }
}

/// In-memory UserOperation mempool.
///
/// `chain_id` is stamped at construction so `add()` and `drain_for_bundle()`
/// always agree on the EntryPoint hash domain. Use `zbx_types::CHAIN_ID_MAINNET`
/// for production and `zbx_types::CHAIN_ID_TESTNET` for testnet+devnet.
pub struct BundlerMempool {
    ops: Arc<RwLock<HashMap<String, UserOperation>>>,
    chain_id: u64,
}

impl BundlerMempool {
    /// Construct a mempool bound to a specific chain ID. The chain ID is
    /// part of the EntryPoint hash domain (ERC-4337) and must match the
    /// chain ID the wallets sign their UserOperations against.
    pub fn new(chain_id: u64) -> Self {
        BundlerMempool {
            ops: Arc::new(RwLock::new(HashMap::new())),
            chain_id,
        }
    }

    /// Add a UserOperation to the mempool. The op is hashed against the
    /// mempool's bound `chain_id` (set at construction).
    pub fn add(&self, op: UserOperation) -> Result<[u8; 32], BundlerError> {
        let hash = op.hash(crate::ENTRY_POINT_ADDRESS, self.chain_id);
        let key = hex::encode(hash);
        self.ops.write().unwrap().insert(key, op);
        Ok(hash)
    }

    /// Get a pending UserOperation by hash.
    pub fn get(&self, hash: &str) -> Option<UserOperation> {
        self.ops.read().unwrap().get(hash).cloned()
    }

    /// Drain up to MAX_BUNDLE_SIZE UserOps for bundling.
    pub fn drain_for_bundle(&self) -> Vec<UserOperation> {
        let mut ops = self.ops.write().unwrap();
        let selected: Vec<_> = ops.values()
            .take(crate::MAX_BUNDLE_SIZE)
            .cloned()
            .collect();
        for op in &selected {
            let key = hex::encode(op.hash(crate::ENTRY_POINT_ADDRESS, self.chain_id));
            ops.remove(&key);
        }
        selected
    }

    /// Current mempool size.
    pub fn len(&self) -> usize {
        self.ops.read().unwrap().len()
    }

    /// Chain ID this mempool is bound to.
    pub fn chain_id(&self) -> u64 {
        self.chain_id
    }
}

// Note: no `Default` impl — chain_id MUST be specified explicitly at construction
// to prevent the S13-CHAIN-ID-DRIFT class of bug (silent default to wrong chain).