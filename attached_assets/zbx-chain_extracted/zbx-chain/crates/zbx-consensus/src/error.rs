use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConsensusError {
    #[error("safety violation: {0}")]
    SafetyViolation(String),

    #[error("round {got} is behind locked round {locked}")]
    StaleRound { got: u64, locked: u64 },

    #[error("quorum certificate has insufficient votes: {got}/{required}")]
    InsufficientVotes { got: usize, required: usize },

    #[error("invalid block proposer for round {0}")]
    InvalidProposer(u64),

    #[error("timeout in round {0}")]
    Timeout(u64),

    #[error("duplicate vote from validator {0}")]
    DuplicateVote(String),

    #[error("block not found: {0}")]
    BlockNotFound(String),

    // ── HotStuff-2 variants (ZEP-022) ────────────────────────────────────────

    #[error("stale proposal: proposal round {proposal_round} is behind highest QC round {highest_qc}")]
    StaleProposal { proposal_round: u64, highest_qc: u64 },

    #[error("invalid timeout certificate: insufficient shares")]
    InvalidTimeoutCertificate,
    // SEC-2026-05-09 (Pass-5 H3): raised when the local node would
    // double-vote at the same HotStuff-2 round on a different block.
    #[error("equivocation: already voted at round {round} on {seen:?}, refused to vote on {attempted:?}")]
    Equivocation {
        round:     u64,
        seen:      zbx_types::H256,
        attempted: zbx_types::H256,
    },

    // SEC-2026-05-09 Pass-10 (architect-review follow-up): a REMOTE
    // validator signed two different block hashes at the same
    // (round, phase). Carries the full slashable evidence so the
    // node-level handler can persist it and bump the metric. The
    // detector lives in `HotStuff2::on_vote` (and `HotStuff::on_vote`)
    // before votes reach the accumulator.
    #[error("remote equivocation: validator {validator:?} signed two hashes at round {round} phase {phase}: {hash_a:?} vs {hash_b:?}")]
    RemoteEquivocation {
        validator: zbx_types::address::Address,
        round:     u64,
        phase:     u8,
        hash_a:    zbx_types::H256,
        hash_b:    zbx_types::H256,
    },

    #[error("consecutive timeouts exceeded maximum: {0}")]
    ConsecutiveTimeoutsExceeded(u64),

    // ── Block-layer safety variants ───────────────────────────────────────────

    /// CSN-QC-01: proposal carries a parent QC with an invalid BLS signature.
    #[error("invalid quorum certificate in proposal for round {0}")]
    InvalidQC(u64),

    // ── N-05 fix: unknown QC phase must be an explicit error, not a silent NOP.
    // A `_ => {}` catch-all in `on_qc()` silently swallowed any QC whose
    // phase byte was not 0, 1, or 2, masking protocol violations and making
    // it impossible to detect a malformed or replayed QC in audit logs.
    #[error("invalid consensus message: {0}")]
    InvalidMessage(String),

    // ── Gossip rate-limiting (raised by `GossipFilter::on_inbound`) ─────────
    /// A peer exceeded its per-second gossip rate budget. The caller
    /// should drop the message and may penalise the peer.
    #[error("gossip rate limit exceeded")]
    RateLimitExceeded,

    // ── Pacemaker / epoch mismatch (raised by `on_timeout_share`) ──────────
    /// A remote timeout-share carried an epoch that does not match the
    /// local pacemaker's current epoch — silently dropping it would
    /// mask cross-epoch replay attempts, so we surface it explicitly.
    #[error("invalid epoch: expected {expected}, got {got}")]
    InvalidEpoch { expected: u64, got: u64 },

    // ── Constructor guard (raised by HotStuff2::try_new / ValidatorSet::try_new) ─
    /// Consensus cannot function with zero validators — constructing
    /// a quorum-based system with an empty set would panic on any
    /// proposer-rotation or quorum-threshold computation.
    #[error("empty validator set: cannot construct consensus with zero validators")]
    EmptyValidatorSet,
}