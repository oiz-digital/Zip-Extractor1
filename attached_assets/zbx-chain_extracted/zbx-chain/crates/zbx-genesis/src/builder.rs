//! GenesisBuilder — constructs the genesis block from a GenesisSpec.

use crate::{GenesisError, GenesisSpec, allocations::parse_allocations};

/// Builds and validates the genesis block.
pub struct GenesisBuilder {
    spec: GenesisSpec,
}

impl GenesisBuilder {
    pub fn new(spec: GenesisSpec) -> Self { Self { spec } }

    /// Load from a JSON file.
    pub fn from_json(path: &str) -> Result<Self, GenesisError> {
        let data = std::fs::read_to_string(path)?;
        let spec: GenesisSpec = serde_json::from_str(&data)?;
        Ok(Self::new(spec))
    }

    /// Validate the spec (sanity checks before building state).
    pub fn validate(&self) -> Result<(), GenesisError> {
        let spec = &self.spec;

        if spec.chain_id == 0 {
            return Err(GenesisError::Invalid("chain_id cannot be 0".into()));
        }
        if spec.gas_limit < 21_000 {
            return Err(GenesisError::Invalid("gas_limit too low".into()));
        }
        if spec.validators.is_empty() {
            return Err(GenesisError::Invalid("no validators in genesis".into()));
        }

        // Validate `alloc` balances do not exceed the portion of the 150 M ZBX
        // hard cap that is reserved for pre-minted genesis wallets.
        //
        // Supply breakdown (mainnet-genesis.json):
        //   Genesis wallet alloc  :  39,990,000 ZBX   (this check)
        //   Validator stakes      :     400,000 ZBX   (4 × 100,000; validated by consensus)
        //   Mining rewards cap    : 109,610,000 ZBX   (block rewards over time)
        //   ─────────────────────────────────────────
        //   Total hard cap        : 150,000,000 ZBX
        //
        // The alloc ceiling (ALLOC_CAP) covers only the genesis wallet portion
        // so that validator stakes and mining rewards still fit under the hard cap.
        let alloc_cap: u128 = 39_990_000 * 10u128.pow(18);
        let total = spec.total_allocated();
        if total > alloc_cap {
            return Err(GenesisError::Invalid(
                format!(
                    "total genesis alloc {total} wei exceeds alloc cap {alloc_cap} wei \
                     (39,990,000 ZBX). Combined with 400,000 ZBX validator stakes and \
                     109,610,000 ZBX mining cap this would exceed the 150 M ZBX hard cap."
                )
            ));
        }

        // Validate all allocations parse correctly.
        parse_allocations(&spec.alloc)?;

        Ok(())
    }

    /// S30 — Build the raw 32-byte state root.
    ///
    /// Replaces the pre-S30 placeholder XOR "hash" with real
    /// `zbx_crypto::keccak256`. Used by [`Self::genesis_block_hash`]
    /// for the canonical chain identifier; also exposed via
    /// [`Self::state_root`] in hex form for human inspection / config
    /// files.
    ///
    /// **Injective encoding (S30-HIGH-2 fix):** commits ALL fields
    /// of every allocation — `address`, `balance`, `nonce`, `code`,
    /// AND `storage` — using length-prefixed framing so two genesis
    /// specs that differ in any byte of code or any storage slot
    /// produce different roots. This was previously broken: only
    /// (address, balance, nonce) were hashed, allowing two different
    /// allocation sets to map to the same `state_root` and therefore
    /// the same `genesis_block_hash`.
    ///
    /// Layout (all multi-byte ints are big-endian):
    ///   account_count        : u64
    ///   for each (address-sorted) account:
    ///     address            : [u8;20]   (fixed)
    ///     balance            : u128       (fixed, 16 bytes)
    ///     nonce              : u64        (fixed)
    ///     code_len           : u64
    ///     code_bytes         : variable
    ///     storage_count      : u64
    ///     for each (key-sorted) slot:
    ///       key              : [u8;32]   (fixed)
    ///       value            : [u8;32]   (fixed)
    pub fn state_root_bytes(&self) -> Result<zbx_types::H256, GenesisError> {
        let accounts = parse_allocations(&self.spec.alloc)?;

        // In production this inserts into a zbx-trie::MerklePatriciaTrie.
        // Here we produce a deterministic, INJECTIVE hash over sorted accounts
        // so the operator-pinned genesis_block_hash uniquely identifies the
        // entire allocation state (including contract code and storage).
        let mut buf = Vec::with_capacity(64 * accounts.len() + 8);
        buf.extend_from_slice(&(accounts.len() as u64).to_be_bytes());
        for acc in &accounts {
            // Fixed-width per-account fields.
            buf.extend_from_slice(&acc.address);                  // 20 fixed
            buf.extend_from_slice(&acc.balance.to_be_bytes());    // 16 fixed (u128)
            buf.extend_from_slice(&acc.nonce.to_be_bytes());      //  8 fixed (u64)

            // Variable: code is length-prefixed (injective).
            buf.extend_from_slice(&(acc.code.len() as u64).to_be_bytes());
            buf.extend_from_slice(&acc.code);

            // Variable: storage map — count + sorted (key,value) pairs.
            // HashMap iteration is non-deterministic; sort by key for
            // canonical encoding.
            let mut slots: Vec<(&[u8; 32], &[u8; 32])> = acc.storage.iter().collect();
            slots.sort_by_key(|(k, _)| **k);
            buf.extend_from_slice(&(slots.len() as u64).to_be_bytes());
            for (k, v) in slots {
                buf.extend_from_slice(k);   // 32 fixed
                buf.extend_from_slice(v);   // 32 fixed
            }
        }
        Ok(zbx_crypto::keccak256(&buf))
    }

    /// Hex-encoded state root for human-readable display (config files,
    /// logs). Internal hashing uses [`Self::state_root_bytes`] so the
    /// canonical genesis hash is over raw bytes, not a hex string
    /// (avoids ambiguous-encoding attacks — see S30 architect notes).
    pub fn state_root(&self) -> Result<String, GenesisError> {
        Ok(format!("0x{}", hex::encode(self.state_root_bytes()?.0)))
    }

    /// S30 — Compute the canonical genesis BLOCK hash (block #0).
    ///
    /// **Injective length-prefixed encoding** (closes S30-HIGH-1):
    /// every variable-length field is preceded by its 8-byte BE
    /// length, and lists are preceded by an 8-byte BE element count.
    /// This guarantees that two distinct [`GenesisSpec`] instances
    /// cannot serialise to the same preimage — a property the
    /// previous concatenation-based encoding lacked (e.g. validator
    /// lists `["ab","c"]` and `["a","bc"]` collided to identical
    /// bytes, breaking the pinned-genesis trust anchor).
    ///
    /// Layout:
    ///   chain_id          : u64  BE  (8 bytes, fixed)
    ///   timestamp         : u64  BE  (8 bytes, fixed)
    ///   gas_limit         : u64  BE  (8 bytes, fixed)
    ///   base_fee_per_gas  : u64  BE  (8 bytes, fixed)
    ///   state_root        : H256     (32 bytes raw, fixed)
    ///   extra_data_len    : u64  BE  (8 bytes)
    ///   extra_data_bytes  : variable
    ///   validators_count  : u64  BE  (8 bytes)
    ///   for each (sorted) validator:
    ///       validator_len    : u64  BE  (8 bytes)
    ///       validator_bytes  : variable
    ///
    /// This is the value the operator pins in
    /// [`zbx_types::pinned_genesis::MAINNET_GENESIS_HASH`] before
    /// mainnet launch.
    pub fn genesis_block_hash(&self) -> Result<zbx_types::H256, GenesisError> {
        let state_root = self.state_root_bytes()?;
        let mut buf = Vec::with_capacity(512);

        // Fixed-width scalars (length is implicit in the type).
        buf.extend_from_slice(&self.spec.chain_id.to_be_bytes());
        buf.extend_from_slice(&self.spec.timestamp.to_be_bytes());
        buf.extend_from_slice(&self.spec.gas_limit.to_be_bytes());
        buf.extend_from_slice(&self.spec.base_fee_per_gas.to_be_bytes());
        buf.extend_from_slice(&state_root.0);

        // Variable-length: length-prefixed (injective).
        let extra = self.spec.extra_data.as_bytes();
        buf.extend_from_slice(&(extra.len() as u64).to_be_bytes());
        buf.extend_from_slice(extra);

        // List: count-prefixed, each item length-prefixed (injective).
        // Sort validators for determinism (HashMap iteration is not stable).
        let mut vs: Vec<&String> = self.spec.validators.iter().collect();
        vs.sort();
        buf.extend_from_slice(&(vs.len() as u64).to_be_bytes());
        for v in vs {
            let vb = v.as_bytes();
            buf.extend_from_slice(&(vb.len() as u64).to_be_bytes());
            buf.extend_from_slice(vb);
        }

        Ok(zbx_crypto::keccak256(&buf))
    }

    pub fn spec(&self) -> &GenesisSpec { &self.spec }

    /// SEC-2026-05-09 Pass-19 (Task #9) — INITIAL EPOCH SEED.
    ///
    /// Derived deterministically from the canonical genesis block hash
    /// + chain ID, so every node bootstraps to the same epoch-0 seed
    /// without needing an out-of-band ceremony. The consensus driver
    /// is expected to call
    /// `HotStuffConsensus::rotate_epoch_seed(genesis_epoch_seed())`
    /// at startup so the very first proposer rotation already uses the
    /// keccak-keyed shuffle (Pass-15 HIGH-R03 path) instead of the
    /// legacy `round % n` fallback.
    ///
    /// Encoding: `keccak256(genesis_block_hash || chain_id_be8)`.
    /// Length-prefix is implicit (32 + 8 fixed bytes), so no
    /// concatenation-ambiguity hazard.
    pub fn genesis_epoch_seed(&self) -> Result<zbx_types::H256, GenesisError> {
        let gh = self.genesis_block_hash()?;
        let mut buf = [0u8; 40];
        buf[..32].copy_from_slice(gh.as_bytes());
        buf[32..40].copy_from_slice(&self.spec.chain_id.to_be_bytes());
        Ok(zbx_crypto::keccak256(&buf))
    }
}

#[cfg(test)]
mod genesis_hash_tests {
    use super::*;
    use crate::spec::{Allocation, ChainConfig};
    use std::collections::HashMap;

    fn empty_spec(validators: Vec<&str>) -> GenesisSpec {
        GenesisSpec {
            name: "test".into(),
            chain_id: 31337,
            timestamp: 1_700_000_000,
            gas_limit: 30_000_000,
            base_fee_per_gas: 1_000_000_000,
            difficulty: 0,
            extra_data: "zbx-test".into(),
            config: ChainConfig {
                chain_id: 31337,
                london_block: 0,
                merge_netsplit_block: None,
                shanghai_block: None,
                cancun_block: None,
            },
            alloc: HashMap::new(),
            validators: validators.into_iter().map(String::from).collect(),
            token_premints: Vec::new(),
        }
    }

    #[test]
    fn pass19_genesis_epoch_seed_is_deterministic_and_chain_dependent() {
        // SEC-2026-05-09 Pass-19 (Task #9): the consensus driver
        // bootstraps `epoch_seed` from this value at startup. Same spec
        // → same seed (every node agrees); different chain ID → different
        // seed (chain-isolation, no cross-chain replay of epoch
        // schedules even with identical validator sets).
        let a1 = GenesisBuilder::new(empty_spec(vec!["v1", "v2"])).genesis_epoch_seed().unwrap();
        let a2 = GenesisBuilder::new(empty_spec(vec!["v1", "v2"])).genesis_epoch_seed().unwrap();
        assert_eq!(a1, a2, "same spec must produce same epoch seed");

        let mut alt = empty_spec(vec!["v1", "v2"]);
        alt.chain_id = 99999;
        alt.config.chain_id = 99999;
        let b = GenesisBuilder::new(alt).genesis_epoch_seed().unwrap();
        assert_ne!(a1, b, "different chain ID must produce different epoch seed");
    }

    #[test]
    fn s30_genesis_hash_is_injective_for_validator_concat_ambiguity() {
        // Pre-S30 encoding mapped these to identical preimages:
        //   ["ab","c"]  →  "ab" + "c"  = "abc"
        //   ["a","bc"]  →  "a"  + "bc" = "abc"
        // Length-prefixed encoding must distinguish them.
        let h_ab_c = GenesisBuilder::new(empty_spec(vec!["ab", "c"]))
            .genesis_block_hash().unwrap();
        let h_a_bc = GenesisBuilder::new(empty_spec(vec!["a", "bc"]))
            .genesis_block_hash().unwrap();
        assert_ne!(h_ab_c, h_a_bc,
            "validator-concat ambiguity must be broken by length prefixes");
    }

    #[test]
    fn s30_genesis_hash_distinguishes_extra_data_from_state_root() {
        // Length prefix must prevent extra_data bytes from sliding into
        // the state_root field's "preimage slot" via concat ambiguity.
        let mut a = empty_spec(vec!["v1"]);
        a.extra_data = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into(); // 32 chars
        let mut b = empty_spec(vec!["v1"]);
        b.extra_data = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAB".into(); // 33 chars
        let ha = GenesisBuilder::new(a).genesis_block_hash().unwrap();
        let hb = GenesisBuilder::new(b).genesis_block_hash().unwrap();
        assert_ne!(ha, hb);
    }

    #[test]
    fn s30_genesis_hash_is_deterministic_across_validator_input_order() {
        // Validators are sorted internally → input order MUST NOT affect hash.
        let h1 = GenesisBuilder::new(empty_spec(vec!["alpha", "beta", "gamma"]))
            .genesis_block_hash().unwrap();
        let h2 = GenesisBuilder::new(empty_spec(vec!["gamma", "alpha", "beta"]))
            .genesis_block_hash().unwrap();
        assert_eq!(h1, h2);
    }

    #[test]
    fn s30_state_root_text_matches_state_root_bytes() {
        let b = GenesisBuilder::new(empty_spec(vec!["v1"]));
        let bytes = b.state_root_bytes().unwrap();
        let text  = b.state_root().unwrap();
        assert_eq!(text, format!("0x{}", hex::encode(bytes.0)));
    }

    fn alloc_with(code: Option<&str>, storage: Option<Vec<(&str, &str)>>) -> HashMap<String, Allocation> {
        let mut m = HashMap::new();
        m.insert("0x0000000000000000000000000000000000000001".into(), Allocation {
            balance: "0x0".into(),
            code:    code.map(String::from),
            storage: storage.map(|v| v.into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()),
            nonce:   0,
        });
        m
    }

    #[test]
    fn s30_state_root_changes_when_only_account_code_differs() {
        // S30 HIGH-2 regression test: code must be committed in state_root.
        let mut s1 = empty_spec(vec!["v1"]);
        s1.alloc = alloc_with(Some("0xdead"), None);
        let mut s2 = empty_spec(vec!["v1"]);
        s2.alloc = alloc_with(Some("0xbeef"), None);
        let h1 = GenesisBuilder::new(s1).state_root_bytes().unwrap();
        let h2 = GenesisBuilder::new(s2).state_root_bytes().unwrap();
        assert_ne!(h1, h2, "code change must affect state_root (HIGH-2 fix)");
    }

    #[test]
    fn s30_state_root_changes_when_only_storage_value_differs() {
        // S30 HIGH-2 regression test: storage must be committed in state_root.
        let mut s1 = empty_spec(vec!["v1"]);
        s1.alloc = alloc_with(None, Some(vec![("0x01", "0xaa")]));
        let mut s2 = empty_spec(vec!["v1"]);
        s2.alloc = alloc_with(None, Some(vec![("0x01", "0xbb")]));
        let h1 = GenesisBuilder::new(s1).state_root_bytes().unwrap();
        let h2 = GenesisBuilder::new(s2).state_root_bytes().unwrap();
        assert_ne!(h1, h2, "storage value change must affect state_root (HIGH-2 fix)");
    }

    #[test]
    fn s30_state_root_changes_when_only_storage_key_differs() {
        // Different key, same value → different state_root.
        let mut s1 = empty_spec(vec!["v1"]);
        s1.alloc = alloc_with(None, Some(vec![("0x01", "0xaa")]));
        let mut s2 = empty_spec(vec!["v1"]);
        s2.alloc = alloc_with(None, Some(vec![("0x02", "0xaa")]));
        let h1 = GenesisBuilder::new(s1).state_root_bytes().unwrap();
        let h2 = GenesisBuilder::new(s2).state_root_bytes().unwrap();
        assert_ne!(h1, h2, "storage key change must affect state_root (HIGH-2 fix)");
    }

    #[test]
    fn s30_state_root_deterministic_across_storage_input_order() {
        // Storage entries are sorted by key internally → input order
        // must NOT affect the resulting state_root.
        let mut s1 = empty_spec(vec!["v1"]);
        s1.alloc = alloc_with(None, Some(vec![("0x01", "0xaa"), ("0x02", "0xbb")]));
        let mut s2 = empty_spec(vec!["v1"]);
        s2.alloc = alloc_with(None, Some(vec![("0x02", "0xbb"), ("0x01", "0xaa")]));
        let h1 = GenesisBuilder::new(s1).state_root_bytes().unwrap();
        let h2 = GenesisBuilder::new(s2).state_root_bytes().unwrap();
        assert_eq!(h1, h2, "storage input order must not change state_root");
    }
}