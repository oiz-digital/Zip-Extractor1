//! Genesis block construction for Zebvix mainnet and testnets.
//!
//! Selects the genesis allocation, builds the genesis `Block` (height 0),
//! and persists it to storage on first boot.
//!
//! ## Audit 2026-04-30 — S4-B3 (HIGH) closed
//!
//! `bootstrap_into` now **fails fast** when the on-disk genesis hash or the
//! persisted chain_id disagrees with the current config. Previously the node
//! would silently keep running with whichever genesis was on disk, which is a
//! catastrophic foot-gun for operators who change `chain_id` or genesis allocs
//! and accidentally fork their own mainnet replica off into a private chain.
//!
//! The legacy "warn-and-continue" behaviour is still reachable behind an
//! explicit `--allow-chain-mismatch` operator override, surfaced via
//! `BootstrapPolicy`. We log loudly when that override is in effect.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use tracing::{info, warn};
use zbx_storage::ZbxDb;
use zbx_types::{
    account::AccountState,
    address::Address,
    block::Block,
    error::ZbxError,
    H256, TOTAL_SUPPLY,
};

const META_KEY_CHAIN_ID: &[u8] = b"chain_id";

/// Policy controlling how `bootstrap_into` reacts to a mismatch between the
/// genesis on disk and the genesis derived from the current config.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BootstrapPolicy {
    /// Hard-fail (default). Production-safe.
    #[default]
    StrictFailFast,
    /// Operator override — log loudly and continue with the on-disk genesis.
    /// Only intended for local recovery / forensic work.
    AllowMismatch,
}

/// Selectable network presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Network {
    Mainnet,
    Testnet,
}

impl Network {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.to_ascii_lowercase().as_str() {
            "mainnet" | "main" => Ok(Network::Mainnet),
            "testnet" | "test" => Ok(Network::Testnet),
            other => Err(format!("unknown network: {other}")),
        }
    }

    pub fn chain_id(self) -> u64 {
        match self {
            Network::Mainnet => zbx_types::CHAIN_ID,    // 8989
            Network::Testnet => zbx_types::CHAIN_ID + 1, // 8990
        }
    }

    pub fn rpc_port(self) -> u16 {
        match self {
            Network::Mainnet => 8545,
            Network::Testnet => 18545,
        }
    }

    pub fn p2p_port(self) -> u16 {
        match self {
            Network::Mainnet => 30303,
            Network::Testnet => 30304,
        }
    }
}

/// Deserialize u128 from either a JSON integer or a decimal string.
///
/// Standard JSON cannot represent integers > 2^53 without losing precision,
/// so genesis files should quote large Wei balances as strings.  This
/// helper accepts both forms so operators have flexibility:
///   `"balance": 1000000000000000000`      (number, ≤ u64::MAX only)
///   `"balance": "10000000000000000000000"` (string, any u128 value)
mod balance_serde {
    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u128, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum BalanceVal {
            Num(u64),
            Str(String),
        }
        match BalanceVal::deserialize(d)? {
            BalanceVal::Num(n) => Ok(n as u128),
            BalanceVal::Str(s) => s.parse::<u128>().map_err(serde::de::Error::custom),
        }
    }
}

/// Genesis account allocation entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenesisAlloc {
    pub address: Address,
    /// Wei balance.  In genesis JSON files write large values as a quoted
    /// decimal string, e.g. `"balance": "10000000000000000000000"`.
    #[serde(deserialize_with = "balance_serde::deserialize")]
    pub balance: u128,
    #[serde(default)]
    pub nonce: u64,
    pub code: Option<String>,
    pub storage: Option<HashMap<String, String>>,
}

/// Genesis configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenesisConfig {
    pub chain_id: u64,
    pub timestamp: u64,
    pub gas_limit: u64,
    pub base_fee: u64,
    pub extra_data: String,
    pub alloc: Vec<GenesisAlloc>,
    pub validators: Vec<Address>,
}

impl GenesisConfig {
    /// Mainnet preset (chain_id 8989).
    pub fn mainnet() -> Result<Self, ZbxError> {
        let allocs = vec![
            // Team multisig — 15M ZBX
            ("0x0000000000000000000000000000000000001001", TOTAL_SUPPLY / 10),
            // Ecosystem fund — 60M ZBX
            ("0x0000000000000000000000000000000000001002", TOTAL_SUPPLY * 4 / 10),
            // Staking rewards reserve — 45M ZBX
            ("0x0000000000000000000000000000000000001003", TOTAL_SUPPLY * 3 / 10),
        ];
        let mut alloc = parse_allocs(&allocs)?;
        // Task #7: pre-deploy ZbxVaultRegistry.sol at the canonical
        // address 0x..5455 so precompile 0x0F has a non-empty code
        // entry to read against. The registry is a minimal
        // storage-only contract — its code is just a STOP sentinel
        // (`0x00`) since the precompile bypasses execution and reads
        // storage slots directly. Writers (legacy ZusdVault → registry
        // migration) populate `cdps[owner]` at slot 0 via SSTORE.
        alloc.push(GenesisAlloc {
            address: Address::from_hex("0x0000000000000000000000000000000000005455")?,
            balance: 0,
            nonce: 1,
            code: Some("0x00".into()),
            storage: None,
        });
        let validators = vec![
            "0x0000000000000000000000000000000000002001",
            "0x0000000000000000000000000000000000002002",
            "0x0000000000000000000000000000000000002003",
            "0x0000000000000000000000000000000000002004",
            "0x0000000000000000000000000000000000002005",
        ];
        Ok(GenesisConfig {
            chain_id: Network::Mainnet.chain_id(),
            timestamp: 1_714_521_600, // 2024-05-01 00:00:00 UTC
            gas_limit: zbx_types::BLOCK_GAS_LIMIT,
            base_fee: 1_000_000_000,
            extra_data: "Zebvix Mainnet Genesis - Building the Future of Finance".into(),
            alloc,
            validators: parse_addresses(&validators)?,
        })
    }

    /// Testnet preset (chain_id 8990).
    pub fn testnet() -> Result<Self, ZbxError> {
        let allocs = vec![
            ("0x0000000000000000000000000000000000003001", TOTAL_SUPPLY / 5),
            ("0x0000000000000000000000000000000000003002", TOTAL_SUPPLY / 5),
        ];
        let mut alloc = parse_allocs(&allocs)?;
        // Task #7: same registry pre-deployment as mainnet.
        alloc.push(GenesisAlloc {
            address: Address::from_hex("0x0000000000000000000000000000000000005455")?,
            balance: 0,
            nonce: 1,
            code: Some("0x00".into()),
            storage: None,
        });
        let validators = vec!["0x0000000000000000000000000000000000004001"];
        Ok(GenesisConfig {
            chain_id: Network::Testnet.chain_id(),
            timestamp: 1_714_521_600,
            gas_limit: zbx_types::BLOCK_GAS_LIMIT,
            base_fee: 1_000_000_000,
            extra_data: "Zebvix Testnet Genesis".into(),
            alloc,
            validators: parse_addresses(&validators)?,
        })
    }

    pub fn for_network(net: Network) -> Result<Self, ZbxError> {
        match net {
            Network::Mainnet => Self::mainnet(),
            Network::Testnet => Self::testnet(),
        }
    }

    /// Resolve the correct genesis preset for a chain id.
    pub fn for_network_id(chain_id: u64) -> Result<Self, ZbxError> {
        if chain_id == Network::Mainnet.chain_id() {
            Self::mainnet()
        } else if chain_id == Network::Testnet.chain_id() {
            Self::testnet()
        } else {
            Err(ZbxError::InvalidHex(format!("unknown chain id: {chain_id}")))
        }
    }

    /// Load a genesis config from a JSON file.
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| format!("genesis read: {e}"))?;
        let cfg: Self = serde_json::from_str(&raw).map_err(|e| format!("genesis parse: {e}"))?;
        cfg.validate_no_placeholders()?;
        Ok(cfg)
    }

    /// P2-PROD: Reject mainnet genesis configs that still contain placeholder
    /// addresses — addresses whose first 18 bytes are all zero indicate a
    /// sequential devnet stub (e.g. `0x0000000000000000000000000000000000002001`).
    /// Real validator and treasury addresses generated by `zbx-keygen generate`
    /// are full-entropy 20-byte values and will never match this pattern.
    ///
    /// Only enforced for `chain_id == 8989` (mainnet). Testnet nodes may freely
    /// use placeholder addresses for local testing.
    pub fn validate_no_placeholders(&self) -> Result<(), String> {
        if self.chain_id != zbx_types::CHAIN_ID {
            return Ok(());
        }

        // An address is a placeholder if:
        //   (a) its first 18 bytes are all zero — sequential devnet stubs like
        //       0x0000000000000000000000000000000000002001, OR
        //   (b) it is the zero address entirely (common copy-paste mistake).
        let is_placeholder = |bytes: &[u8; 20]| {
            bytes.iter().all(|&b| b == 0)          // zero address
            || bytes[..18].iter().all(|&b| b == 0) // sequential stub
        };

        // OPERATOR-05: validators is Vec<Address> in GenesisConfig.
        // Check every validator address is non-placeholder.
        for (i, v) in self.validators.iter().enumerate() {
            if is_placeholder(v.as_bytes()) {
                return Err(format!(
                    "OPERATOR-05: mainnet genesis validator[{i}] has a placeholder address \
                     0x{} (zero or near-zero bytes). \
                     Replace with a real secp256k1 address from `zbx-keygen generate`. \
                     See docs/VALIDATOR_GUIDE.md.",
                    hex::encode(v.as_bytes())
                ));
            }
        }
        for a in &self.alloc {
            if is_placeholder(a.address.as_bytes()) {
                return Err(format!(
                    "OPERATOR-05: mainnet genesis alloc contains a placeholder address \
                     0x{} (zero or near-zero bytes). \
                     Replace all treasury/fund addresses with real wallet addresses \
                     before mainnet launch.",
                    hex::encode(a.address.as_bytes())
                ));
            }
        }
        Ok(())
    }

    /// Build the genesis `Block` (height 0) plus the initial account-state list.
    pub fn build_genesis_block(&self) -> (Block, Vec<(Address, AccountState)>) {
        let mut accounts = Vec::with_capacity(self.alloc.len());
        for a in &self.alloc {
            let mut s = AccountState::default();
            s.set_balance_u128(a.balance);
            s.nonce = a.nonce;
            accounts.push((a.address, s));
        }
        let state_root = compute_genesis_state_root(&accounts);
        let block = Block::genesis(state_root, self.timestamp);
        (block, accounts)
    }

    /// On first boot: write genesis block + allocations to storage.
    /// Returns Ok((true, hash)) if genesis was just created, Ok((false, hash))
    /// if a previously persisted genesis was found and reused.
    ///
    /// Hard-fails on hash / chain_id mismatch unless `policy ==
    /// BootstrapPolicy::AllowMismatch` (set via `--allow-chain-mismatch`).
    pub fn bootstrap_into(
        &self,
        db: &ZbxDb,
        policy: BootstrapPolicy,
    ) -> Result<(bool, H256), String> {
        // P2-PROD: Refuse to boot mainnet with placeholder validator/alloc
        // addresses regardless of whether the genesis was loaded from a file
        // or from the built-in preset. The operator MUST replace the stubs
        // with real keys before a mainnet launch. Skippable only with
        // --allow-chain-mismatch (which also gates all other strict checks).
        if policy == BootstrapPolicy::StrictFailFast {
            self.validate_no_placeholders()?;
        } else {
            // AllowMismatch: still warn, but continue.
            if let Err(e) = self.validate_no_placeholders() {
                warn!("P2-PROD warning (overridden by --allow-chain-mismatch): {e}");
            }
        }

        // ─── 1. Persisted-genesis path ────────────────────────────────────
        if let Some(existing) = db.genesis().map_err(|e| format!("read genesis: {e}"))? {
            let existing_hash = existing.hash();
            let (computed, _) = self.build_genesis_block();
            let computed_hash = computed.hash();

            if existing_hash != computed_hash {
                let msg = format!(
                    "GENESIS MISMATCH — on-disk hash 0x{} differs from config-derived 0x{}. \
                     Likely causes: chain_id changed, alloc edited, or wrong data dir. \
                     Wipe the data dir to start a fresh chain, OR re-run with \
                     --allow-chain-mismatch to keep the existing chain.",
                    hex::encode(existing_hash),
                    hex::encode(computed_hash),
                );
                if policy == BootstrapPolicy::AllowMismatch {
                    warn!(
                        on_disk = %hex::encode(existing_hash),
                        computed = %hex::encode(computed_hash),
                        "GENESIS MISMATCH overridden by --allow-chain-mismatch — \
                         continuing with on-disk genesis. Production operators MUST \
                         NOT keep this flag set; it bypasses fork-safety guarantees."
                    );
                } else {
                    return Err(msg);
                }
            } else {
                info!(hash = %hex::encode(existing_hash), "found existing genesis on disk");
            }

            // ─── 2. Persisted chain_id check ───────────────────────────
            // We persisted chain_id in metadata on first boot. A drift here
            // means the operator changed --network or chain.chain_id without
            // wiping the data dir — that would silently fork the local node
            // off the real chain.
            if let Some(stored) = db
                .get_metadata(META_KEY_CHAIN_ID)
                .map_err(|e| format!("read chain_id metadata: {e}"))?
            {
                if stored.len() == 8 {
                    let mut bytes = [0u8; 8];
                    bytes.copy_from_slice(&stored);
                    let stored_id = u64::from_be_bytes(bytes);
                    if stored_id != self.chain_id {
                        let msg = format!(
                            "CHAIN_ID MISMATCH — data dir was bootstrapped with chain_id {stored_id} \
                             but config requests chain_id {}. Wipe data dir or re-run with \
                             --allow-chain-mismatch.",
                            self.chain_id,
                        );
                        if policy == BootstrapPolicy::AllowMismatch {
                            warn!(
                                stored = stored_id,
                                requested = self.chain_id,
                                "CHAIN_ID MISMATCH overridden by --allow-chain-mismatch"
                            );
                        } else {
                            return Err(msg);
                        }
                    }
                } else {
                    warn!(
                        len = stored.len(),
                        "stored chain_id metadata is not 8 bytes — ignoring (will be \
                         rewritten on next genesis bootstrap)"
                    );
                }
            } else {
                // Older data dirs pre-date the metadata field. Backfill it
                // now so subsequent boots get the strict check.
                db.put_metadata(META_KEY_CHAIN_ID, self.chain_id.to_be_bytes().to_vec())
                    .map_err(|e| format!("backfill chain_id metadata: {e}"))?;
            }

            return Ok((false, existing_hash));
        }

        // ─── 3. Fresh chain path ──────────────────────────────────────────
        let (block, accounts) = self.build_genesis_block();
        let hash = block.hash();
        db.put_block(&block).map_err(|e| format!("put genesis block: {e}"))?;
        for (addr, state) in &accounts {
            db.put_account(addr, state)
                .map_err(|e| format!("put genesis account: {e}"))?;
        }
        db.put_metadata(META_KEY_CHAIN_ID, self.chain_id.to_be_bytes().to_vec())
            .map_err(|e| format!("put chain_id: {e}"))?;
        info!(
            hash = %hex::encode(hash),
            chain_id = self.chain_id,
            allocs = self.alloc.len(),
            validators = self.validators.len(),
            "genesis block written to storage"
        );
        Ok((true, hash))
    }
}

fn parse_allocs(items: &[(&str, u128)]) -> Result<Vec<GenesisAlloc>, ZbxError> {
    items
        .iter()
        .map(|(addr, balance)| {
            Ok(GenesisAlloc {
                address: Address::from_hex(addr)?,
                balance: *balance,
                nonce: 0,
                code: None,
                storage: None,
            })
        })
        .collect()
}

fn parse_addresses(items: &[&str]) -> Result<Vec<Address>, ZbxError> {
    items.iter().map(|a| Address::from_hex(a)).collect()
}

/// Compute the genesis state root using a real Merkle-Patricia Trie
/// (S33-state-root W4).
///
/// Was a flat keccak placeholder over `addr || nonce || balance || code_hash`
/// — which (a) omitted `storage_root`, and (b) was order-dependent on the
/// caller-supplied slice. The new implementation delegates to
/// `zbx_state::mpt::compute_state_root` so it produces a canonical Yellow
/// Paper §4.1 root identical to what `StateDB::state_root()` and
/// `StateView::state_root()` produce for the same accounts.
///
/// # Genesis-hash compatibility note
///
/// The genesis state root WILL CHANGE for any chain that previously relied
/// on the flat-keccak placeholder. For Zebvix's pre-launch state this is
/// acceptable: there is no production chain to preserve compatibility with.
/// The chain-mismatch hard-fail in `bootstrap_into` (Audit S4-B3) will
/// catch any operator running an old binary against a new genesis.
///
/// # Storage at genesis
///
/// This function ignores `GenesisAlloc.storage` and uses each account's
/// `storage_root` field as-is. Pre-populated genesis storage (if any) must
/// already be reflected in the supplied `AccountState.storage_root` value
/// before calling. For the current devnet/testnet/mainnet specs the
/// `storage` field is unused and all accounts have `EMPTY_STORAGE_ROOT`,
/// so this restriction has no practical impact today.
fn compute_genesis_state_root(accounts: &[(Address, AccountState)]) -> H256 {
    let mut map: HashMap<Address, AccountState> = HashMap::new();
    for (addr, state) in accounts {
        map.insert(*addr, state.clone());
    }
    let storage: HashMap<Address, HashMap<H256, H256>> = HashMap::new();
    zbx_state::mpt::compute_state_root(&map, &storage)
}
