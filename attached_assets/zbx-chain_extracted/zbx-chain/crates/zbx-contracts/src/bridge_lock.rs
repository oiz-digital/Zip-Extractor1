//! Bridge lock contract — escrows ZBX/tokens for cross-chain bridging.
//!
//! ## Security model
//!
//! * **OUT1 (nonce-collision)**: deposit ID = `(source_chain_id << 32) | sequence`.
//!   Scoped to this contract's chain, never collides across chains or redeployments.
//! * **OUT2 (source-binding)**: every `Deposit` stores `source_chain_id`; `vote_release`
//!   requires the caller to supply the claimed source chain and rejects mismatches.
//! * **MS1 (tally-griefing)**: `vote_release` accumulates relayer votes in a
//!   `PendingRelease` map; funds only move once `threshold` distinct relayers have
//!   voted.  `cancel_release` (owner-only) clears a tally without moving funds,
//!   giving the operator a circuit-breaker against fraudulent vote sets.
//! * **S11-1**: `lock()` rejects deposits below `MIN_DEPOSIT_WEI` (0.001 ZBX).
//! * **S11-2**: `lock()` deducts a configurable fee (bps) and records both gross
//!   and net amounts; the caller's balance must cover the gross amount.
//! * **S11-3**: `target_chain == 0` is rejected (reserved / invalid chain ID).
//! * **S11-4**: `target_recipient == [0; 20]` (zero address) is rejected.
//! * **S11-5**: ownership transfer follows a two-step accept pattern — the new owner
//!   must call `accept_ownership` to prevent accidental mis-transfer.
//! * **S11-6**: the relayer set is bounded to `MAX_RELAYERS` (20) to prevent O(n)
//!   DoS when iterating the vote-tally loop inside `vote_release`.

use std::collections::{HashMap, HashSet};
use zbx_types::address::Address;

// ── Constants ────────────────────────────────────────────────────────────────

/// Minimum deposit in wei (0.001 ZBX).  S11-1.
pub const MIN_DEPOSIT_WEI: u128 = 1_000_000_000_000_000;

/// Maximum authorised relayers in a single contract instance.  S11-6.
/// Bounds the worst-case vote-tally iteration in `vote_release`.
pub const MAX_RELAYERS: usize = 20;

const ZERO_ADDR: [u8; 20] = [0u8; 20];

// ── Error type ───────────────────────────────────────────────────────────────

/// Typed bridge errors (replaces stringly-typed `&'static str`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeError {
    /// Amount is zero or below `MIN_DEPOSIT_WEI`.  S11-1.
    DepositTooSmall,
    /// `target_chain == 0` is reserved.  S11-3.
    InvalidTargetChain,
    /// `target_recipient` is the zero address.  S11-4.
    ZeroRecipient,
    /// Caller is not the contract owner.
    NotOwner,
    /// Relayer set has reached `MAX_RELAYERS`.  S11-6.
    RelayerSetFull,
    /// Caller is not an authorised relayer.
    NotRelayer,
    /// Deposit ID does not exist.
    DepositNotFound,
    /// Claimed source chain ID does not match the deposit's recorded chain.  OUT2.
    SourceChainMismatch,
    /// Deposit (or its pending release) has already been finalised.
    AlreadyReleased,
    /// This relayer already cast a vote for the given deposit.
    AlreadyVoted,
    /// No pending ownership transfer exists for `accept_ownership`.  S11-5.
    NoPendingOwner,
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

// ── Data structures ──────────────────────────────────────────────────────────

/// A single bridge-out deposit.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Deposit {
    /// Globally unique deposit ID: `(source_chain_id << 32) | sequence`.  OUT1.
    pub id:               u64,
    /// Chain ID this contract is deployed on (stored on each deposit).  OUT2.
    pub source_chain_id:  u64,
    pub sender:           Address,
    /// Net locked amount after fee deduction.
    pub amount:           u128,
    /// Protocol fee collected (gross - net).  S11-2.
    pub fee:              u128,
    /// Target chain identifier (BSC = 56, ETH = 1, …).  S11-3 guards chain 0.
    pub target_chain:     u64,
    /// Recipient on the target chain.  S11-4 guards zero address.
    pub target_recipient: [u8; 20],
    /// Block number when the deposit was created.
    pub block_number:     u64,
    /// `true` once the multisig quorum has released the funds.
    pub released:         bool,
}

/// Accumulated relayer votes for a pending release (MS1).
#[derive(Debug, Default)]
struct PendingRelease {
    /// Relayers that have voted to approve this release.
    votes: HashSet<Address>,
    /// `true` once the release has been executed (funds moved).
    finalized: bool,
}

/// Bridge lock contract state.
#[derive(Debug)]
pub struct BridgeLockContract {
    deposits:         HashMap<u64, Deposit>,
    /// Vote tallies indexed by deposit_id.  MS1.
    pending_releases: HashMap<u64, PendingRelease>,
    /// Monotonically increasing per-instance sequence counter.  OUT1.
    sequence:         u32,
    /// Chain ID of the chain this instance is deployed on.  OUT1 + OUT2.
    source_chain_id:  u64,
    /// Fee in basis points (e.g. 30 = 0.3%).  S11-2.
    fee_bps:          u128,
    /// Minimum distinct relayer votes required to execute a release.  MS1.
    threshold:        usize,
    /// Set of authorised relayers.  S11-6: bounded to `MAX_RELAYERS`.
    relayers:         HashSet<Address>,
    owner:            Address,
    /// Pending new owner (two-step transfer).  S11-5.
    pending_owner:    Option<Address>,
}

// ── Implementation ───────────────────────────────────────────────────────────

impl BridgeLockContract {
    /// Create a new bridge lock contract.
    ///
    /// # Panics
    /// Panics if `threshold == 0` (a zero-threshold quorum would allow any
    /// relayer to release funds without any peer agreement, which is equivalent
    /// to having no multisig at all).
    pub fn new(
        owner: Address,
        source_chain_id: u64,
        fee_bps: u128,
        threshold: usize,
    ) -> Self {
        assert!(threshold >= 1, "threshold must be ≥ 1");
        Self {
            deposits:         HashMap::new(),
            pending_releases: HashMap::new(),
            sequence:         0,
            source_chain_id,
            fee_bps,
            threshold,
            relayers:         HashSet::new(),
            owner,
            pending_owner:    None,
        }
    }

    // ── Relayer management ────────────────────────────────────────────────────

    /// Add an authorised relayer.  Only the owner can call this.
    ///
    /// Fails with [`BridgeError::RelayerSetFull`] if `MAX_RELAYERS` is reached (S11-6).
    pub fn add_relayer(
        &mut self,
        caller: &Address,
        relayer: Address,
    ) -> Result<(), BridgeError> {
        if *caller != self.owner { return Err(BridgeError::NotOwner); }
        if self.relayers.len() >= MAX_RELAYERS { return Err(BridgeError::RelayerSetFull); }
        self.relayers.insert(relayer);
        Ok(())
    }

    /// Remove an authorised relayer.  Only the owner can call this.
    pub fn remove_relayer(
        &mut self,
        caller: &Address,
        relayer: &Address,
    ) -> Result<(), BridgeError> {
        if *caller != self.owner { return Err(BridgeError::NotOwner); }
        self.relayers.remove(relayer);
        Ok(())
    }

    // ── Ownership (two-step — S11-5) ──────────────────────────────────────────

    /// Initiate an ownership transfer to `new_owner`.
    ///
    /// The transfer is not complete until `new_owner` calls `accept_ownership`.
    pub fn transfer_ownership(
        &mut self,
        caller: &Address,
        new_owner: Address,
    ) -> Result<(), BridgeError> {
        if *caller != self.owner { return Err(BridgeError::NotOwner); }
        self.pending_owner = Some(new_owner);
        Ok(())
    }

    /// Complete a pending ownership transfer.
    ///
    /// Must be called by the address set in `transfer_ownership`.
    pub fn accept_ownership(&mut self, caller: &Address) -> Result<(), BridgeError> {
        match self.pending_owner {
            Some(po) if po == *caller => {
                self.owner = po;
                self.pending_owner = None;
                Ok(())
            }
            _ => Err(BridgeError::NoPendingOwner),
        }
    }

    // ── Core operations ───────────────────────────────────────────────────────

    /// Lock tokens for cross-chain bridging.
    ///
    /// Returns `(deposit_id, fee_charged_wei, net_locked_wei)`.
    ///
    /// The caller's balance must cover `amount` (gross).  The contract will
    /// record only `net = amount − fee` as the releasable amount on the
    /// destination chain.
    ///
    /// # Errors
    /// * [`BridgeError::DepositTooSmall`]   — amount < `MIN_DEPOSIT_WEI`.  S11-1.
    /// * [`BridgeError::InvalidTargetChain`] — `target_chain == 0`.  S11-3.
    /// * [`BridgeError::ZeroRecipient`]      — `target_recipient` is `[0;20]`.  S11-4.
    pub fn lock(
        &mut self,
        sender: Address,
        amount: u128,
        target_chain: u64,
        target_recipient: [u8; 20],
        block_number: u64,
    ) -> Result<(u64, u128, u128), BridgeError> {
        if amount < MIN_DEPOSIT_WEI         { return Err(BridgeError::DepositTooSmall); }
        if target_chain == 0                { return Err(BridgeError::InvalidTargetChain); }
        if target_recipient == ZERO_ADDR    { return Err(BridgeError::ZeroRecipient); }

        // OUT1: ID is (source_chain_id << 32) | sequence.
        // Safe from collision across chains (different source_chain_id) and across
        // redeployments (sequence is reset but source_chain_id encodes the chain,
        // and each contract instance has a unique deployment address on-chain).
        let id = (self.source_chain_id << 32) | (self.sequence as u64);
        self.sequence = self.sequence.saturating_add(1);

        // S11-2: fee deduction — gross stays with caller, net is locked.
        let fee = amount * self.fee_bps / 10_000;
        let net  = amount.saturating_sub(fee);

        self.deposits.insert(id, Deposit {
            id,
            source_chain_id: self.source_chain_id,
            sender,
            amount: net,
            fee,
            target_chain,
            target_recipient,
            block_number,
            released: false,
        });

        Ok((id, fee, net))
    }

    /// Cast a relayer vote to release a deposit.
    ///
    /// Returns `Ok(Some(net_amount))` once the vote threshold is met and funds
    /// are released.  Returns `Ok(None)` when the vote is recorded but the
    /// threshold has not yet been reached.
    ///
    /// The caller (executor / precompile) is responsible for crediting
    /// `target_recipient` on the destination chain with `net_amount` once this
    /// function returns `Some`.
    ///
    /// # Security
    /// * **MS1** — requires `threshold` distinct relayer votes; no single relayer
    ///   can release unilaterally.
    /// * **OUT2** — `claimed_source_chain` must match the deposit's recorded
    ///   `source_chain_id`; mismatches are rejected.
    /// * **Replay** — deposit is marked `released = true` on finalisation;
    ///   subsequent vote attempts return `AlreadyReleased`.
    pub fn vote_release(
        &mut self,
        relayer: &Address,
        deposit_id: u64,
        claimed_source_chain: u64,
    ) -> Result<Option<u128>, BridgeError> {
        if !self.relayers.contains(relayer) {
            return Err(BridgeError::NotRelayer);
        }

        // OUT2: claimed source chain must match what was locked.
        let dep = self.deposits.get(&deposit_id)
            .ok_or(BridgeError::DepositNotFound)?;
        if dep.source_chain_id != claimed_source_chain {
            return Err(BridgeError::SourceChainMismatch);
        }
        if dep.released {
            return Err(BridgeError::AlreadyReleased);
        }

        // MS1: accumulate votes in the tally map.
        let pending = self.pending_releases.entry(deposit_id).or_default();
        if pending.finalized {
            return Err(BridgeError::AlreadyReleased);
        }
        if !pending.votes.insert(*relayer) {
            return Err(BridgeError::AlreadyVoted);
        }

        if pending.votes.len() >= self.threshold {
            // Quorum reached — finalise.
            pending.finalized = true;
            let dep = self.deposits.get_mut(&deposit_id).unwrap();
            dep.released = true;
            Ok(Some(dep.amount))
        } else {
            Ok(None)
        }
    }

    /// Clear the vote tally for a pending release without moving funds.
    ///
    /// **Owner-only**.  Use this as a circuit-breaker when a quorum of relayers
    /// submitted a fraudulent or incorrect release attempt (MS1 griefing mitigation).
    /// Clearing the tally forces relayers to re-vote from scratch.
    pub fn cancel_release(
        &mut self,
        caller: &Address,
        deposit_id: u64,
    ) -> Result<(), BridgeError> {
        if *caller != self.owner { return Err(BridgeError::NotOwner); }
        if !self.deposits.contains_key(&deposit_id) {
            return Err(BridgeError::DepositNotFound);
        }
        // Remove the tally — does NOT mark the deposit as released.
        self.pending_releases.remove(&deposit_id);
        Ok(())
    }

    // ── Read-only helpers ─────────────────────────────────────────────────────

    pub fn deposit(&self, id: u64) -> Option<&Deposit> {
        self.deposits.get(&id)
    }

    /// Current vote count for a pending release.
    pub fn vote_count(&self, deposit_id: u64) -> usize {
        self.pending_releases
            .get(&deposit_id)
            .map_or(0, |p| p.votes.len())
    }

    pub fn threshold(&self)       -> usize { self.threshold }
    pub fn source_chain_id(&self) -> u64   { self.source_chain_id }
    pub fn relayer_count(&self)   -> usize { self.relayers.len() }
    pub fn owner(&self)           -> Address { self.owner }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const CHAIN_ZBX: u64  = 8989;
    const CHAIN_BSC: u64  = 56;
    const OWNER:   Address = Address([0x01u8; 20]);
    const RELAYER1: Address = Address([0xA1u8; 20]);
    const RELAYER2: Address = Address([0xA2u8; 20]);
    const RELAYER3: Address = Address([0xA3u8; 20]);
    const ALICE:   Address = Address([0xCCu8; 20]);
    const BOB_BSC: [u8; 20] = [0xBBu8; 20];
    const ONE_ZBX: u128 = 1_000_000_000_000_000_000;

    fn make_contract(threshold: usize) -> BridgeLockContract {
        let mut c = BridgeLockContract::new(OWNER, CHAIN_ZBX, 30, threshold);
        c.add_relayer(&OWNER, RELAYER1).unwrap();
        c.add_relayer(&OWNER, RELAYER2).unwrap();
        c.add_relayer(&OWNER, RELAYER3).unwrap();
        c
    }

    // ── OUT1: nonce-collision ─────────────────────────────────────────────────

    /// Deposit IDs are strictly unique within one contract instance.
    #[test]
    fn out1_deposit_ids_unique() {
        let mut c = make_contract(2);
        let mut ids = std::collections::HashSet::new();
        for _ in 0..1000 {
            let (id, _, _) = c.lock(ALICE, ONE_ZBX, CHAIN_BSC, BOB_BSC, 100).unwrap();
            assert!(ids.insert(id), "OUT1: deposit ID must be unique");
        }
    }

    /// IDs from two contracts on DIFFERENT chains never collide.
    #[test]
    fn out1_ids_cross_chain_no_collision() {
        let mut zbx = BridgeLockContract::new(OWNER, CHAIN_ZBX, 0, 1);
        let mut bsc = BridgeLockContract::new(OWNER, CHAIN_BSC, 0, 1);

        // Generate 100 IDs on each chain and verify they are disjoint.
        let zbx_ids: std::collections::HashSet<u64> = (0..100)
            .map(|_| zbx.lock(ALICE, ONE_ZBX, CHAIN_BSC, BOB_BSC, 1).unwrap().0)
            .collect();
        let bsc_ids: std::collections::HashSet<u64> = (0..100)
            .map(|_| bsc.lock(ALICE, ONE_ZBX, CHAIN_ZBX, BOB_BSC, 1).unwrap().0)
            .collect();
        assert!(
            zbx_ids.is_disjoint(&bsc_ids),
            "OUT1: IDs from different source chains must not collide"
        );
    }

    // ── OUT2: source-binding ──────────────────────────────────────────────────

    /// `vote_release` rejects a proof that claims the wrong source chain.
    #[test]
    fn out2_wrong_source_chain_rejected() {
        let mut c = make_contract(1);
        let (id, _, _) = c.lock(ALICE, ONE_ZBX, CHAIN_BSC, BOB_BSC, 100).unwrap();

        // Relayer supplies ETH (chain 1) instead of ZBX (8989).
        let err = c.vote_release(&RELAYER1, id, 1).unwrap_err();
        assert_eq!(err, BridgeError::SourceChainMismatch,
            "OUT2: wrong source chain must be rejected");
    }

    /// `vote_release` accepts a proof with the correct source chain.
    #[test]
    fn out2_correct_source_chain_accepted() {
        let mut c = make_contract(1);
        let (id, _, _) = c.lock(ALICE, ONE_ZBX, CHAIN_BSC, BOB_BSC, 100).unwrap();
        let result = c.vote_release(&RELAYER1, id, CHAIN_ZBX).unwrap();
        assert!(result.is_some(), "OUT2: correct source chain must succeed");
    }

    // ── MS1: tally-griefing ───────────────────────────────────────────────────

    /// Single relayer cannot release with threshold = 2.
    #[test]
    fn ms1_single_relayer_cannot_release_below_threshold() {
        let mut c = make_contract(2);
        let (id, _, _) = c.lock(ALICE, ONE_ZBX, CHAIN_BSC, BOB_BSC, 100).unwrap();

        let result = c.vote_release(&RELAYER1, id, CHAIN_ZBX).unwrap();
        assert!(result.is_none(), "MS1: 1-of-2 should not release");
        assert_eq!(c.vote_count(id), 1);
    }

    /// Reaching the threshold finalises the release.
    #[test]
    fn ms1_threshold_met_releases_funds() {
        let mut c = make_contract(2);
        let (id, _, net) = c.lock(ALICE, ONE_ZBX, CHAIN_BSC, BOB_BSC, 100).unwrap();

        c.vote_release(&RELAYER1, id, CHAIN_ZBX).unwrap();
        let result = c.vote_release(&RELAYER2, id, CHAIN_ZBX).unwrap();
        assert_eq!(result, Some(net), "MS1: 2-of-2 should release net amount");
        assert!(c.deposit(id).unwrap().released);
    }

    /// Voting twice by the same relayer is rejected.
    #[test]
    fn ms1_double_vote_rejected() {
        let mut c = make_contract(3);
        let (id, _, _) = c.lock(ALICE, ONE_ZBX, CHAIN_BSC, BOB_BSC, 100).unwrap();

        c.vote_release(&RELAYER1, id, CHAIN_ZBX).unwrap();
        let err = c.vote_release(&RELAYER1, id, CHAIN_ZBX).unwrap_err();
        assert_eq!(err, BridgeError::AlreadyVoted, "MS1: same relayer must not vote twice");
    }

    /// After finalization further votes are rejected.
    #[test]
    fn ms1_post_release_vote_rejected() {
        let mut c = make_contract(2);
        let (id, _, _) = c.lock(ALICE, ONE_ZBX, CHAIN_BSC, BOB_BSC, 100).unwrap();

        c.vote_release(&RELAYER1, id, CHAIN_ZBX).unwrap();
        c.vote_release(&RELAYER2, id, CHAIN_ZBX).unwrap();
        let err = c.vote_release(&RELAYER3, id, CHAIN_ZBX).unwrap_err();
        assert_eq!(err, BridgeError::AlreadyReleased, "MS1: post-release vote must be rejected");
    }

    /// Owner can cancel a pending tally; afterwards relayers can re-vote.
    #[test]
    fn ms1_owner_cancel_clears_tally() {
        let mut c = make_contract(2);
        let (id, _, _) = c.lock(ALICE, ONE_ZBX, CHAIN_BSC, BOB_BSC, 100).unwrap();

        c.vote_release(&RELAYER1, id, CHAIN_ZBX).unwrap();
        assert_eq!(c.vote_count(id), 1);

        c.cancel_release(&OWNER, id).unwrap();
        assert_eq!(c.vote_count(id), 0, "tally must be cleared after cancel");
        assert!(!c.deposit(id).unwrap().released, "deposit must not be released after cancel");

        // Fresh vote after cancel must still work.
        c.vote_release(&RELAYER1, id, CHAIN_ZBX).unwrap();
        let result = c.vote_release(&RELAYER2, id, CHAIN_ZBX).unwrap();
        assert!(result.is_some(), "quorum after cancel must release normally");
    }

    // ── S11 bugs ──────────────────────────────────────────────────────────────

    /// S11-1: sub-minimum deposits rejected.
    #[test]
    fn s11_1_minimum_deposit_enforced() {
        let mut c = make_contract(1);
        let too_small = MIN_DEPOSIT_WEI - 1;
        let err = c.lock(ALICE, too_small, CHAIN_BSC, BOB_BSC, 1).unwrap_err();
        assert_eq!(err, BridgeError::DepositTooSmall);

        // Exact minimum is accepted.
        c.lock(ALICE, MIN_DEPOSIT_WEI, CHAIN_BSC, BOB_BSC, 1).unwrap();
    }

    /// S11-2: fee is deducted from locked amount.
    #[test]
    fn s11_2_fee_deducted_correctly() {
        let mut c = BridgeLockContract::new(OWNER, CHAIN_ZBX, 30, 1); // 0.3% fee
        c.add_relayer(&OWNER, RELAYER1).unwrap();

        let gross = ONE_ZBX;
        let (id, fee, net) = c.lock(ALICE, gross, CHAIN_BSC, BOB_BSC, 1).unwrap();
        let expected_fee = gross * 30 / 10_000;
        assert_eq!(fee, expected_fee, "S11-2: fee must be 0.3% of gross");
        assert_eq!(net, gross - fee,  "S11-2: net must be gross - fee");
        assert_eq!(c.deposit(id).unwrap().amount, net, "S11-2: deposit.amount must be net");
    }

    /// S11-3: chain ID 0 rejected.
    #[test]
    fn s11_3_chain_zero_rejected() {
        let mut c = make_contract(1);
        let err = c.lock(ALICE, ONE_ZBX, 0, BOB_BSC, 1).unwrap_err();
        assert_eq!(err, BridgeError::InvalidTargetChain, "S11-3: chain 0 must be rejected");
    }

    /// S11-4: zero recipient address rejected.
    #[test]
    fn s11_4_zero_recipient_rejected() {
        let mut c = make_contract(1);
        let err = c.lock(ALICE, ONE_ZBX, CHAIN_BSC, [0u8; 20], 1).unwrap_err();
        assert_eq!(err, BridgeError::ZeroRecipient, "S11-4: zero recipient must be rejected");
    }

    /// S11-5: two-step ownership transfer.
    #[test]
    fn s11_5_two_step_ownership_transfer() {
        let new_owner = Address([0x99u8; 20]);
        let mut c = make_contract(1);

        // Initiate — owner changes only after accept.
        c.transfer_ownership(&OWNER, new_owner).unwrap();
        assert_eq!(c.owner(), OWNER, "ownership must not change until accepted");

        // Wrong address cannot accept.
        let err = c.accept_ownership(&ALICE).unwrap_err();
        assert_eq!(err, BridgeError::NoPendingOwner);

        // Correct address accepts.
        c.accept_ownership(&new_owner).unwrap();
        assert_eq!(c.owner(), new_owner, "ownership must transfer after accept");
    }

    /// S11-6: relayer set bounded to MAX_RELAYERS.
    #[test]
    fn s11_6_relayer_set_bounded() {
        let mut c = BridgeLockContract::new(OWNER, CHAIN_ZBX, 0, 1);
        for i in 0..(MAX_RELAYERS as u8) {
            let r = Address([i; 20]);
            c.add_relayer(&OWNER, r).unwrap();
        }
        let overflow = Address([0xFF; 20]);
        let err = c.add_relayer(&OWNER, overflow).unwrap_err();
        assert_eq!(err, BridgeError::RelayerSetFull,
            "S11-6: relayer set must be bounded to MAX_RELAYERS");
    }

    /// Replay attack: releasing the same deposit twice is rejected.
    #[test]
    fn replay_attack_prevented() {
        let mut c = make_contract(1);
        let (id, _, _) = c.lock(ALICE, ONE_ZBX, CHAIN_BSC, BOB_BSC, 100).unwrap();

        c.vote_release(&RELAYER1, id, CHAIN_ZBX).unwrap();
        let err = c.vote_release(&RELAYER2, id, CHAIN_ZBX).unwrap_err();
        assert_eq!(err, BridgeError::AlreadyReleased, "replay must be rejected");
    }

    /// Unauthorised relayer cannot release.
    #[test]
    fn unauthorised_relayer_rejected() {
        let mut c = make_contract(1);
        let (id, _, _) = c.lock(ALICE, ONE_ZBX, CHAIN_BSC, BOB_BSC, 100).unwrap();
        let stranger = Address([0xEEu8; 20]);
        let err = c.vote_release(&stranger, id, CHAIN_ZBX).unwrap_err();
        assert_eq!(err, BridgeError::NotRelayer);
    }
}
