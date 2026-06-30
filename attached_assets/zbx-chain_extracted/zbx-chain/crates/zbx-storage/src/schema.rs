//! Column family names and key encoding conventions.

/// All RocksDB column families used by ZBX.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Column {
    /// block_hash (32 bytes) → serialised Block
    Blocks,
    /// block_number (8 BE bytes) → block_hash (32 bytes)
    BlockNumbers,
    /// tx_hash (32 bytes) → serialised SignedTransaction
    Transactions,
    /// tx_hash (32 bytes) → serialised TransactionReceipt
    Receipts,
    /// keccak256(address) (32 bytes) → serialised AccountState
    State,
    /// keccak256(address || slot) (32 bytes) → slot value (32 bytes)
    Storage,
    /// code_hash (32 bytes) → EVM bytecode
    Code,
    /// Named metadata keys (string → bytes)
    Metadata,
    /// Merkle-Patricia Trie internal nodes (S33-state-root W3b).
    /// Key   = keccak256(node_RLP) (32 bytes)
    /// Value = RLP-encoded node (variable length)
    /// Used by `ZbxDbTrieAdapter` (in zbx-state) to back the canonical
    /// world-state and per-account storage tries persistently.
    TrieNodes,
    // SEC-2026-05-09 Pass-11 — slashing pipeline persistence.
    //
    // Pre-Pass-11 the consensus driver detected remote validator
    // equivocation and `tracing::error!`'d "SLASHABLE" — but the
    // evidence was discarded on process restart and never reached
    // `SlashingRegistryV2`. Without persistence, the chain had a
    // detector but no economic security. These two CFs close that
    // hole.
    /// Raw `EquivocationEvidence` records emitted by the consensus
    /// detector, keyed by `keccak256(bincode(evidence))`. Entries are
    /// idempotent — re-detecting the same equivocation is a no-op.
    /// Records here are the *input* to the slashing pipeline.
    /// Key   = 32-byte content hash
    /// Value = bincode(EquivocationEvidence)
    SlashingEvidence,
    /// `SlashEvidenceRecord` snapshots from `SlashingRegistryV2`
    /// after submission. Persisted on every state transition
    /// (Pending → Appealed → Confirmed/Overturned) so a node restart
    /// rehydrates the registry exactly. Whistleblower bonds + epoch
    /// counters live here too (under fixed metadata keys, see
    /// `db::PIPELINE_META_*`).
    /// Key   = 32-byte SlashEvidenceRecord.id
    /// Value = bincode(SlashEvidenceRecord)
    SlashingRecords,
    // On-chain staking transaction pipeline.
    //
    // Pre-Task-1, `ValidatorSet::delegate` only tracked an aggregate
    // `delegated_stake: u128` per validator with no per-delegator
    // breakdown — meaning a delegator could never undelegate their
    // own share, only the validator could "burn" the lump sum. These
    // two CFs back the per-delegator delegation ledger and the
    // 21-day unbonding queue.
    //
    /// Per-(validator, delegator) outstanding delegation amount.
    /// Key   = validator(20) ‖ delegator(20)   = 40 bytes
    /// Value = u128 LE bytes (16)
    Delegations,
    /// 21-day unbonding queue keyed by maturity height so the
    /// producer can iterate matured entries with a single prefix
    /// scan up to `current_height_be`.
    /// Key   = unlock_height_be(8) ‖ delegator(20) ‖ validator(20) = 48 bytes
    /// Value = u128 LE bytes (16)
    Unbonding,
    /// Per-epoch `ValidatorSet` snapshot.
    /// Key   = epoch_be(8)
    /// Value = bincode(ValidatorSet)
    ValidatorSets,
    /// Slashing whistleblower / appeal bond ledger.
    ///
    /// Pre-upgrade the `SlashingRegistryV2.pending_bonds` map was in
    /// process memory only — a crash between bond admission and slash
    /// finalization lost the deposit, so non-validator whistleblowers
    /// effectively forfeited their 100 ZBX even though the registry
    /// counted them as still-bonded. Appeal bonds had no escrow at
    /// all (operator-only appeal flow).
    ///
    /// This CF persists every active bond keyed by
    /// `record_id (32) ‖ reporter_address (20)` so a single record
    /// can carry one appeal bond (from the offender) AND N
    /// whistleblower bonds (one per co-witness reporter). Iteration
    /// uses a 32-byte prefix scan on `record_id` to enumerate all
    /// bonds tied to a given slash.
    ///
    /// Key   = record_id(32) ‖ reporter(20)   = 52 bytes
    /// Value = bincode(BondEntry { wei: u128, kind: BondKind })
    ///
    /// Writes are fsynced — losing a bond mid-window is silently the
    /// same as losing the slashing evidence itself.
    SlashingBonds,
    /// Cross-chain bridge: replay-protection spent-operations log.
    ///
    /// Each entry records a `msg_hash` (the keccak256 content-hash of a bridge
    /// execution) that has been fully executed and must never be re-executed.
    ///
    /// Key   = msg_hash (32 bytes) — the same hash passed to
    ///         `MultisigAuth::verify_and_consume` / `mark_spent`.
    /// Value = `[1u8]`             — presence marker (key existence is the
    ///         signal; the value is unused but must be non-empty so the
    ///         WriteBatch layer does not interpret it as a delete).
    ///
    /// Written with fsync (`write_synced`) inside `put_bridge_spent_op` so
    /// a crash between the DB write and the in-memory `mark_spent` still
    /// leaves the hash durably recorded.  On the next node startup
    /// `iter_bridge_spent_ops` rehydrates `MultisigAuth::spent_operations`
    /// so replay is blocked even after a restart.
    BridgeSpentOps,
    /// On-chain governance proposal store (H-4 fix: 2026-06-27).
    ///
    /// Pre-fix, proposals were held in a `HashMap` in `RpcState` — a process
    /// restart wiped every pending proposal. This CF backs the in-memory map
    /// so proposals survive restarts and are consistent across RPC nodes
    /// sharing the same RocksDB data directory.
    ///
    /// Key   = proposalId string bytes (UTF-8, "0x" + 16 hex chars, 34 bytes)
    /// Value = JSON-serialised proposal object (serde_json → UTF-8 bytes)
    ///
    /// Written with `write_synced` (fsync) so a crash in `zbx_proposeGovernance`
    /// never silently drops a submitted proposal.
    GovernanceProposals,
}

impl Column {
    pub fn name(&self) -> &'static str {
        match self {
            Column::Blocks           => "blocks",
            Column::BlockNumbers     => "block_numbers",
            Column::Transactions     => "transactions",
            Column::Receipts         => "receipts",
            Column::State            => "state",
            Column::Storage          => "storage",
            Column::Code             => "code",
            Column::Metadata         => "metadata",
            Column::TrieNodes        => "trie_nodes",
            Column::SlashingEvidence => "slashing_evidence",
            Column::SlashingRecords  => "slashing_records",
            Column::SlashingBonds        => "slashing_bonds",
            Column::Delegations          => "delegations",
            Column::Unbonding            => "unbonding",
            Column::ValidatorSets        => "validator_sets",
            Column::BridgeSpentOps       => "bridge_spent_ops",
            Column::GovernanceProposals  => "governance_proposals",
        }
    }

    pub fn all() -> &'static [Column] {
        &[
            Column::Blocks,
            Column::BlockNumbers,
            Column::Transactions,
            Column::Receipts,
            Column::State,
            Column::Storage,
            Column::Code,
            Column::Metadata,
            Column::TrieNodes,
            Column::SlashingEvidence,
            Column::SlashingRecords,
            Column::SlashingBonds,
            Column::Delegations,
            Column::Unbonding,
            Column::ValidatorSets,
            Column::BridgeSpentOps,
            Column::GovernanceProposals,
        ]
    }
}

// ---------------------------------------------------------------------------
// Key encoding helpers
// ---------------------------------------------------------------------------

pub fn block_key(hash: &[u8; 32]) -> &[u8; 32] { hash }
pub fn block_number_key(n: u64) -> [u8; 8] { n.to_be_bytes() }
pub fn tx_key(hash: &[u8; 32]) -> &[u8; 32] { hash }

pub fn state_key(addr: &[u8; 20]) -> [u8; 32] {
    zbx_crypto::keccak::keccak256(addr).into()
}

pub fn storage_key(addr: &[u8; 20], slot: &[u8; 32]) -> [u8; 32] {
    let mut buf = [0u8; 52];
    buf[..20].copy_from_slice(addr);
    buf[20..].copy_from_slice(slot);
    zbx_crypto::keccak::keccak256(&buf).into()
}