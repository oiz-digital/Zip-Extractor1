//! Light header chain: verify and store block headers.
//!
//! # S26 hardening pass
//!
//! Prior to S26 the QC (quorum certificate) check was a length warning that
//! NEVER rejected a malformed/forged header — any aggregate-signature byte
//! blob would be accepted. S26 wires the chain into
//! `zbx_crypto::bls::verify_aggregate`, which performs the real BLS12-381
//! pairing equation `e(g1, σ) == e(Σ pk_i, H(msg))`.
//!
//! The light client now refuses to insert any non-genesis header whose QC:
//!   - is not exactly 96 bytes (compressed G2 BLS signature length), OR
//!   - does not BLS-verify against the configured validator-set committee.
//!
//! The validator-set is supplied at construction time via `with_validators`
//! and is updateable through `set_validators` (e.g. on epoch boundaries).
//! Without a validator set the chain operates in "header-shape-only" mode
//! (basic field validation, no QC verification) and emits a startup warning.

use zbx_types::H256;
use zbx_types::pinned_genesis::{
    pinned_for, is_sentinel, is_all_zero, PinError, PinPolicy, verify_pinned_with_policy,
};
use zbx_crypto::bls::{BlsPubKey, BlsSignature, verify_aggregate};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use tracing::{info, warn, debug, error};

/// A light block header (stripped for light clients).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LightHeader {
    pub number:           u64,
    pub hash:             H256,
    pub parent_hash:      H256,
    pub state_root:       H256,
    pub transactions_root: H256,
    pub receipts_root:    H256,
    pub timestamp:        u64,
    pub coinbase:         zbx_types::Address,
    /// Aggregate BLS QC: 96-byte compressed G2 signature over the header hash
    /// from the active validator committee (2f+1 threshold).
    pub quorum_cert:      Vec<u8>,
    /// Finalized flag (set by chain after QC chain analysis).
    pub finalized:        bool,
}

impl LightHeader {
    /// Basic shape checks (field invariants only — does NOT check signatures).
    pub fn validate_basic(&self) -> Result<(), &'static str> {
        if self.number == 0 && self.parent_hash != H256::zero() {
            return Err("genesis must have zero parent hash");
        }
        Ok(())
    }
}

/// A trusted checkpoint: a block hash at a known height.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub number: u64,
    pub hash:   H256,
}

/// Genesis checkpoint placeholder — DEPRECATED in S30.
///
/// Use [`HeaderChain::with_pinned_chain_id`] instead, which sources
/// the canonical genesis hash from
/// [`zbx_types::pinned_genesis`] and HARD-rejects sentinel/zero
/// values at startup. The placeholder remains for callers that still
/// want shape-only mode (devnet local-test).
#[deprecated(
    since = "0.2.0",
    note = "Use HeaderChain::with_pinned_chain_id (zbx_types::pinned_genesis) for production. \
            Direct use of this all-zero placeholder is rejected by the pinning enforcer."
)]
pub const GENESIS_CHECKPOINT: Checkpoint = Checkpoint {
    number: 0,
    hash:   H256([0u8; 32]),
};

/// The light client's header chain (in-memory, pruned to last N headers).
pub struct HeaderChain {
    headers:       BTreeMap<u64, LightHeader>,
    canonical:     BTreeMap<u64, H256>, // number → canonical hash
    max_headers:   usize,
    checkpoint:    Checkpoint,
    /// Active validator committee BLS public keys (2f+1 threshold). When
    /// empty, QC verification is skipped (header-shape-only mode).
    validators:    Vec<BlsPubKey>,
    /// S30 MED-2 hard enforcement (closes round-3 architect finding):
    /// trust-anchor flag set ONLY by [`Self::set_devnet_genesis`] in
    /// devnet `AllowUnregistered` mode (or implicitly by mainnet/
    /// testnet auto-pinning). When `false`, [`Self::insert`] refuses
    /// ALL calls — including `header.number == 0` — so the
    /// trust-anchor cannot be installed via the generic insert path,
    /// only via the validating `set_devnet_genesis` (which rejects
    /// sentinel and all-zero).
    ///
    /// Default for legacy `new()` is `true` for backwards-compat.
    devnet_seeded: bool,
}

impl HeaderChain {
    pub fn new(checkpoint: Checkpoint, max_headers: usize) -> Self {
        let hc = Self {
            headers: BTreeMap::new(),
            canonical: BTreeMap::new(),
            max_headers,
            checkpoint,
            validators: Vec::new(),
            // Legacy constructor — preserve pre-S30 behaviour by treating
            // the chain as already seeded. Production callers must
            // migrate to `with_pinned_chain_id` for hard pinning.
            devnet_seeded: true,
        };
        info!("light: header chain initialized from checkpoint #{}", hc.checkpoint.number);
        if hc.checkpoint.hash == H256::zero() {
            warn!("light: GENESIS_CHECKPOINT placeholder hash in use — production deployments \
                   MUST configure the real genesis hash before trusting any sync output");
        }
        hc
    }

    /// S30 — Construct a HeaderChain with the canonical pinned genesis
    /// hash for `chain_id`. HARD-rejects:
    ///   - unknown chain_id (under [`PinPolicy::Required`])
    ///   - sentinel pinned constant (operator forgot to pin)
    ///   - all-zero pinned constant (mis-initialised registry)
    ///
    /// Use [`PinPolicy::AllowUnregistered`] for devnet / local-test
    /// chain IDs (e.g. 31337). Sentinel rejection STILL applies for
    /// known chain IDs even under that policy — operator cannot
    /// accidentally bypass mainnet pinning.
    ///
    /// **Devnet behaviour (S30 fix for MED-2):** under
    /// `AllowUnregistered` for an unknown chain_id, NO genesis row is
    /// auto-inserted. The caller MUST call
    /// [`Self::set_devnet_genesis`] with a non-zero, non-sentinel
    /// hash before any non-genesis header can be accepted. This
    /// prevents the all-zero placeholder from leaking into canonical
    /// state on devnet.
    pub fn with_pinned_chain_id(
        chain_id:        u64,
        max_headers:     usize,
        validators:      Vec<BlsPubKey>,
        policy:          PinPolicy,
    ) -> Result<Self, PinError> {
        let pinned_opt: Option<H256> = match (pinned_for(chain_id), policy) {
            (Some(p), _) => {
                if is_sentinel(&p) {
                    error!(
                        "light: chain_id {} pinned genesis is still SENTINEL — \
                         REFUSING to start. Operator must replace the constant in \
                         zbx_types::pinned_genesis before deploying.", chain_id);
                    return Err(PinError::Sentinel(chain_id));
                }
                if is_all_zero(&p) {
                    return Err(PinError::AllZero(chain_id));
                }
                Some(p)
            }
            (None, PinPolicy::AllowUnregistered) => {
                warn!("light: chain_id {} is unregistered — running in \
                       devnet mode. Caller MUST invoke set_devnet_genesis() \
                       with a non-zero hash before accepting non-genesis headers.",
                      chain_id);
                None
            }
            (None, PinPolicy::Required) => {
                return Err(PinError::UnknownChainId(chain_id));
            }
        };

        // Checkpoint hash mirrors pinned_opt: zero placeholder for devnet
        // (until set_devnet_genesis is called), real hash for mainnet/testnet.
        let checkpoint = Checkpoint {
            number: 0,
            hash:   pinned_opt.unwrap_or(H256::zero()),
        };
        let mut hc = Self {
            headers: BTreeMap::new(),
            canonical: BTreeMap::new(),
            max_headers,
            checkpoint,
            validators,
            // S30 MED-2 round-3: devnet path starts UNSEEDED — the
            // generic insert() path is locked until set_devnet_genesis
            // is called with a validated (non-sentinel, non-zero) hash.
            // Mainnet/testnet path is implicitly seeded because we
            // pin a real hash below.
            devnet_seeded: pinned_opt.is_some(),
        };
        if !hc.validators.is_empty() {
            info!("light: header chain initialized for chain_id {} with {} validators",
                chain_id, hc.validators.len());
        } else {
            warn!("light: header chain initialized for chain_id {} WITHOUT validators \
                   (header-shape-only mode)", chain_id);
        }
        // Auto-seed genesis row ONLY when we have a real pinned hash.
        // Devnet path leaves the chain empty until set_devnet_genesis().
        if let Some(p) = pinned_opt {
            hc.headers.insert(0, LightHeader {
                number: 0, hash: p, parent_hash: H256::zero(),
                state_root: H256::zero(), transactions_root: H256::zero(),
                receipts_root: H256::zero(), timestamp: 0,
                coinbase: zbx_types::Address::zero(),
                quorum_cert: vec![], finalized: true,
            });
            hc.canonical.insert(0, p);
        }
        Ok(hc)
    }

    /// S30 — Devnet genesis seeder. Call after constructing with
    /// [`PinPolicy::AllowUnregistered`] to install the canonical
    /// devnet genesis hash. Rejects sentinel and all-zero so a buggy
    /// caller cannot install placeholder values.
    pub fn set_devnet_genesis(&mut self, hash: H256) -> Result<(), PinError> {
        if is_sentinel(&hash) {
            return Err(PinError::Sentinel(0));
        }
        if is_all_zero(&hash) {
            return Err(PinError::AllZero(0));
        }
        self.checkpoint = Checkpoint { number: 0, hash };
        self.headers.insert(0, LightHeader {
            number: 0, hash, parent_hash: H256::zero(),
            state_root: H256::zero(), transactions_root: H256::zero(),
            receipts_root: H256::zero(), timestamp: 0,
            coinbase: zbx_types::Address::zero(),
            quorum_cert: vec![], finalized: true,
        });
        self.canonical.insert(0, hash);
        // S30 MED-2 round-3: flip the trust-anchor flag — this is the
        // ONLY place that may set devnet_seeded=true on a devnet chain.
        self.devnet_seeded = true;
        info!("light: devnet genesis seeded — chain ready to accept non-genesis headers");
        Ok(())
    }

    /// S30 — Verify a candidate genesis hash matches the pinned
    /// constant for `chain_id`. Wraps
    /// [`zbx_types::pinned_genesis::verify_pinned_with_policy`].
    pub fn verify_pinned_genesis(
        chain_id: u64,
        computed: H256,
        policy:   PinPolicy,
    ) -> Result<(), PinError> {
        verify_pinned_with_policy(chain_id, computed, policy)
    }

    /// Construct with an active validator committee for QC verification.
    pub fn with_validators(checkpoint: Checkpoint, max_headers: usize, validators: Vec<BlsPubKey>) -> Self {
        let mut hc = Self::new(checkpoint, max_headers);
        hc.set_validators(validators);
        hc
    }

    /// Replace the active validator committee (e.g. on epoch rotation).
    pub fn set_validators(&mut self, validators: Vec<BlsPubKey>) {
        if validators.is_empty() {
            warn!("light: validator set cleared — QC verification disabled");
        } else {
            info!("light: validator committee updated, size={}", validators.len());
        }
        self.validators = validators;
    }

    /// Whether QC verification is active (i.e. validator set is configured).
    pub fn has_validators(&self) -> bool {
        !self.validators.is_empty()
    }

    /// Insert and verify a new header.
    pub fn insert(&mut self, header: LightHeader) -> Result<(), &'static str> {
        header.validate_basic()?;

        // S30 MED-2 round-3 hard enforcement: in devnet AllowUnregistered
        // mode the chain is constructed with `devnet_seeded = false`.
        // ALL inserts (including `header.number == 0`) are refused
        // until `set_devnet_genesis()` has installed a validated
        // (non-sentinel, non-zero) trust anchor. This closes the
        // round-3 bypass where an attacker could call
        // `insert(header_at_#0_with_arbitrary_hash)` to prime the
        // chain and then insert non-genesis headers, sidestepping
        // the validation in `set_devnet_genesis`.
        if !self.devnet_seeded {
            return Err("genesis not seeded — call set_devnet_genesis() first");
        }

        // Verify parent linkage.
        if header.number > self.checkpoint.number {
            if let Some(parent) = self.get(header.number - 1) {
                if parent.hash != header.parent_hash {
                    return Err("parent hash mismatch");
                }
            }
        }

        // QC verification: compulsory for non-genesis when a validator set is configured.
        if header.number > 0 {
            if self.has_validators() {
                if !self.verify_qc(&header) {
                    error!("light: header #{} REJECTED — BLS QC verification failed", header.number);
                    return Err("invalid quorum certificate");
                }
            } else {
                warn!("light: header #{} accepted WITHOUT QC check (no validator set)", header.number);
            }
        }

        debug!("light: inserted header #{} {:?}", header.number, header.hash);
        self.canonical.insert(header.number, header.hash);
        self.headers.insert(header.number, header);

        // Prune old headers.
        if self.headers.len() > self.max_headers {
            if let Some(&oldest) = self.headers.keys().next() {
                self.headers.remove(&oldest);
                self.canonical.remove(&oldest);
            }
        }

        Ok(())
    }

    /// Verify the aggregate BLS QC against the active validator committee.
    ///
    /// Expected QC encoding: a 96-byte compressed G2 signature, computed
    /// as the aggregate of validator signatures over `header.hash`.
    fn verify_qc(&self, header: &LightHeader) -> bool {
        if header.quorum_cert.len() != 96 {
            return false;
        }
        let mut sig_bytes = [0u8; 96];
        sig_bytes.copy_from_slice(&header.quorum_cert);
        let sig = BlsSignature(sig_bytes);
        verify_aggregate(&sig, &self.validators, &header.hash)
    }

    pub fn get(&self, number: u64) -> Option<&LightHeader> {
        self.headers.get(&number)
    }

    pub fn tip(&self) -> Option<&LightHeader> {
        self.headers.values().next_back()
    }

    pub fn tip_number(&self) -> u64 {
        self.tip().map(|h| h.number).unwrap_or(self.checkpoint.number)
    }

    pub fn get_by_hash(&self, hash: &H256) -> Option<&LightHeader> {
        self.headers.values().find(|h| &h.hash == hash)
    }

    pub fn len(&self) -> usize { self.headers.len() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbx_crypto::bls::BlsPrivKey;
    use rand::rngs::OsRng;

    fn make_header(number: u64, parent_hash: H256, qc: Vec<u8>) -> LightHeader {
        LightHeader {
            number,
            hash: H256([number as u8; 32]),
            parent_hash,
            state_root: H256::zero(),
            transactions_root: H256::zero(),
            receipts_root: H256::zero(),
            timestamp: 1000 * number,
            coinbase: zbx_types::Address::zero(),
            quorum_cert: qc,
            finalized: false,
        }
    }

    #[test]
    fn header_shape_only_mode_warns_but_inserts() {
        let mut hc = HeaderChain::new(
            Checkpoint { number: 0, hash: H256::zero() },
            100,
        );
        // Genesis-shaped header — parent_hash zero is required at number 0.
        let h0 = make_header(0, H256::zero(), vec![]);
        hc.insert(h0).expect("genesis should insert");

        // No validators → header #1 inserts even with bogus QC (shape-only mode).
        let h1 = make_header(1, H256([0u8; 32]), vec![0xab; 96]);
        hc.insert(h1).expect("shape-only mode accepts header without QC check");
    }

    #[test]
    fn bad_qc_rejected_when_validators_present() {
        let sk1 = BlsPrivKey::generate(&mut OsRng);
        let sk2 = BlsPrivKey::generate(&mut OsRng);
        let pks = vec![sk1.to_pubkey(), sk2.to_pubkey()];

        let mut hc = HeaderChain::with_validators(
            Checkpoint { number: 0, hash: H256::zero() },
            100,
            pks,
        );
        let h0 = make_header(0, H256::zero(), vec![]);
        hc.insert(h0).expect("genesis should insert");

        // Bogus QC bytes — must be rejected.
        let bad = make_header(1, H256([0u8; 32]), vec![0xde; 96]);
        let err = hc.insert(bad).expect_err("bad QC must be rejected");
        assert_eq!(err, "invalid quorum certificate");

        // Wrong-length QC — also rejected.
        let short = make_header(1, H256([0u8; 32]), vec![0xde; 64]);
        let err = hc.insert(short).expect_err("short QC must be rejected");
        assert_eq!(err, "invalid quorum certificate");
    }

    #[test]
    fn valid_aggregate_qc_accepted() {
        // 2-of-2 aggregate signature over the header hash.
        let sk1 = BlsPrivKey::generate(&mut OsRng);
        let sk2 = BlsPrivKey::generate(&mut OsRng);
        let pks = vec![sk1.to_pubkey(), sk2.to_pubkey()];

        let mut hc = HeaderChain::with_validators(
            Checkpoint { number: 0, hash: H256::zero() },
            100,
            pks,
        );
        hc.insert(make_header(0, H256::zero(), vec![])).unwrap();

        let header = make_header(1, H256([0u8; 32]), vec![]);
        let sig1 = sk1.sign(&header.hash);
        let sig2 = sk2.sign(&header.hash);
        let agg = zbx_crypto::bls::aggregate_signatures(&[sig1, sig2]).unwrap();
        let mut signed = header;
        signed.quorum_cert = agg.0.to_vec();

        hc.insert(signed).expect("valid aggregate QC must verify");
        assert_eq!(hc.tip_number(), 1);
    }

    // ─── S30 — Genesis pinning rejections ────────────────────────────

    use zbx_types::CHAIN_ID_MAINNET;

    #[test]
    fn s30_with_pinned_required_rejects_unknown_chain() {
        // HeaderChain has no Debug derive, so unwrap_err() can't be used.
        // Match on the Result directly (S30 LOW-3 fix).
        let result = HeaderChain::with_pinned_chain_id(
            31337, 100, vec![], PinPolicy::Required,
        );
        assert!(matches!(result, Err(PinError::UnknownChainId(31337))));
    }

    #[test]
    fn s30_with_pinned_required_rejects_sentinel_for_mainnet() {
        // Mainnet const is sentinel until operator pins → must hard-error.
        let result = HeaderChain::with_pinned_chain_id(
            CHAIN_ID_MAINNET, 100, vec![], PinPolicy::Required,
        );
        assert!(matches!(result, Err(PinError::Sentinel(CHAIN_ID_MAINNET))));
    }

    #[test]
    fn s30_with_pinned_allow_unregistered_does_not_auto_seed_genesis() {
        // S30 MED-2 fix: devnet construction must NOT auto-insert a
        // zero-hash genesis row into canonical state.
        let hc = HeaderChain::with_pinned_chain_id(
            31337, 100, vec![], PinPolicy::AllowUnregistered,
        ).expect("devnet must construct under permissive policy");
        // No headers should be present until set_devnet_genesis is called.
        assert!(hc.headers.is_empty(), "devnet must NOT auto-seed genesis row");
        assert!(hc.canonical.is_empty(), "devnet must NOT auto-seed canonical row");
    }

    #[test]
    fn s30_set_devnet_genesis_installs_real_hash() {
        let mut hc = HeaderChain::with_pinned_chain_id(
            31337, 100, vec![], PinPolicy::AllowUnregistered,
        ).unwrap();
        let real = H256([0x33u8; 32]);
        hc.set_devnet_genesis(real).expect("non-zero non-sentinel must seed");
        assert_eq!(hc.headers.get(&0).map(|h| h.hash), Some(real));
        assert_eq!(hc.canonical.get(&0).copied(), Some(real));
    }

    #[test]
    fn s30_set_devnet_genesis_rejects_zero_and_sentinel() {
        let mut hc = HeaderChain::with_pinned_chain_id(
            31337, 100, vec![], PinPolicy::AllowUnregistered,
        ).unwrap();
        assert!(matches!(
            hc.set_devnet_genesis(H256::zero()),
            Err(PinError::AllZero(0))
        ));
        assert!(matches!(
            hc.set_devnet_genesis(zbx_types::pinned_genesis::SENTINEL_HASH),
            Err(PinError::Sentinel(0))
        ));
    }

    #[test]
    fn s30_with_pinned_allow_unregistered_still_rejects_sentinel_for_mainnet() {
        // Operator cannot bypass mainnet pinning by flipping the policy flag.
        let result = HeaderChain::with_pinned_chain_id(
            CHAIN_ID_MAINNET, 100, vec![], PinPolicy::AllowUnregistered,
        );
        assert!(matches!(result, Err(PinError::Sentinel(CHAIN_ID_MAINNET))));
    }

    #[test]
    fn s30_devnet_insert_rejects_non_genesis_before_seeding() {
        // S30 MED-2 enforcement: HeaderChain in AllowUnregistered devnet
        // mode with NO seeded genesis must refuse all non-genesis inserts.
        let mut hc = HeaderChain::with_pinned_chain_id(
            31337, 100, vec![], PinPolicy::AllowUnregistered,
        ).expect("devnet must construct under permissive policy");
        // Try to insert header #1 before seeding genesis.
        let h1 = LightHeader {
            number: 1, hash: H256([0x11u8; 32]),
            parent_hash: H256::zero(),
            state_root: H256::zero(),
            transactions_root: H256::zero(),
            receipts_root: H256::zero(),
            timestamp: 1, coinbase: zbx_types::Address::zero(),
            quorum_cert: vec![], finalized: false,
        };
        let result = hc.insert(h1);
        assert!(result.is_err(),
            "must refuse non-genesis insert before set_devnet_genesis()");
        assert!(result.unwrap_err().contains("genesis not seeded"),
            "error must mention genesis-not-seeded reason");
    }

    #[test]
    fn s30_devnet_manual_genesis_insert_does_not_bypass_seeding_requirement() {
        // S30 MED-2 round-3 hardening regression test: an attacker MUST
        // NOT be able to bypass set_devnet_genesis() validation by
        // manually inserting header #0 with an arbitrary (zero, sentinel,
        // or attacker-chosen) hash via the generic insert() path.
        let mut hc = HeaderChain::with_pinned_chain_id(
            31337, 100, vec![], PinPolicy::AllowUnregistered,
        ).expect("devnet must construct");
        // Attempt 1: try to inject genesis #0 directly (bypassing
        // set_devnet_genesis's sentinel/zero validation).
        let h0 = LightHeader {
            number: 0, hash: H256([0x99u8; 32]),
            parent_hash: H256::zero(),
            state_root: H256::zero(),
            transactions_root: H256::zero(),
            receipts_root: H256::zero(),
            timestamp: 0, coinbase: zbx_types::Address::zero(),
            quorum_cert: vec![], finalized: true,
        };
        let r0 = hc.insert(h0);
        assert!(r0.is_err(),
            "manual #0 insert MUST be refused before set_devnet_genesis()");
        assert!(r0.unwrap_err().contains("genesis not seeded"),
            "error must mention genesis-not-seeded reason");
        // Attempt 2: try a sentinel-hash genesis insert directly.
        let h0_sentinel = LightHeader {
            number: 0, hash: zbx_types::pinned_genesis::SENTINEL_HASH,
            parent_hash: H256::zero(),
            state_root: H256::zero(),
            transactions_root: H256::zero(),
            receipts_root: H256::zero(),
            timestamp: 0, coinbase: zbx_types::Address::zero(),
            quorum_cert: vec![], finalized: true,
        };
        let r_sentinel = hc.insert(h0_sentinel);
        assert!(r_sentinel.is_err(),
            "manual sentinel-#0 insert MUST be refused");
        // Verify state is unchanged (no headers, no canonical entries).
        assert!(hc.headers.is_empty(),
            "rejected inserts must not mutate header map");
        assert!(hc.canonical.is_empty(),
            "rejected inserts must not mutate canonical map");
    }

    #[test]
    fn s30_legacy_new_constructor_remains_seeded_for_backcompat() {
        // The legacy `new()` constructor must keep its pre-S30 behaviour
        // (devnet_seeded=true) so existing code paths (sub-modules,
        // older tests, snapshot recovery) continue to work without
        // churn. Hard pinning is opt-in via with_pinned_chain_id.
        let mut hc = HeaderChain::new(
            Checkpoint { number: 0, hash: H256::zero() },
            100,
        );
        let h0 = LightHeader {
            number: 0, hash: H256([0xa1u8; 32]),
            parent_hash: H256::zero(),
            state_root: H256::zero(),
            transactions_root: H256::zero(),
            receipts_root: H256::zero(),
            timestamp: 0, coinbase: zbx_types::Address::zero(),
            quorum_cert: vec![], finalized: true,
        };
        hc.insert(h0).expect(
            "legacy new() must accept inserts (devnet_seeded=true by default)");
    }

    #[test]
    fn s30_devnet_insert_works_after_seeding_genesis() {
        // Companion to the above: after set_devnet_genesis() succeeds,
        // non-genesis inserts may proceed (subject to parent linkage).
        let mut hc = HeaderChain::with_pinned_chain_id(
            31337, 100, vec![], PinPolicy::AllowUnregistered,
        ).expect("devnet must construct");
        let genesis_hash = H256([0x77u8; 32]);
        hc.set_devnet_genesis(genesis_hash).expect("seed must succeed");
        let h1 = LightHeader {
            number: 1, hash: H256([0x88u8; 32]),
            parent_hash: genesis_hash,  // links to seeded genesis
            state_root: H256::zero(),
            transactions_root: H256::zero(),
            receipts_root: H256::zero(),
            timestamp: 1, coinbase: zbx_types::Address::zero(),
            quorum_cert: vec![], finalized: false,
        };
        // No validators → QC check skipped, header-shape-only mode.
        hc.insert(h1).expect("seeded devnet must accept linked non-genesis header");
    }

    #[test]
    fn s30_verify_pinned_genesis_helper_matches_module_fn() {
        let arbitrary = H256([0x42u8; 32]);
        // Mainnet is sentinel → must reject through the helper too.
        assert!(matches!(
            HeaderChain::verify_pinned_genesis(
                CHAIN_ID_MAINNET, arbitrary, PinPolicy::Required,
            ),
            Err(PinError::Sentinel(CHAIN_ID_MAINNET))
        ));
        // Devnet under permissive policy → Ok.
        HeaderChain::verify_pinned_genesis(
            31337, arbitrary, PinPolicy::AllowUnregistered,
        ).unwrap();
    }
}
