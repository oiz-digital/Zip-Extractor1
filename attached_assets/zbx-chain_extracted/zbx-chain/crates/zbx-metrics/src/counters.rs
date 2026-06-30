//! Atomic metric counters and gauges.
//!
//! SEC-2026-05-09 Pass-10: prior `render_prometheus` emitted the literal
//! two-character sequence `\n` (backslash + 'n') in the format string instead
//! of a real newline, producing a single-line response that no Prometheus
//! scraper could parse. Fixed plus expanded coverage:
//!
//!   * BlockMetrics: `last_committed_unix_ms`, `state_root_mismatch_total`,
//!     derived `last_committed_age_seconds` gauge in render output.
//!   * ConsensusMetrics: `quorum_total`, `last_qc_size`, derived
//!     `qc_participation_ratio`.
//!   * RpcMetrics, BridgeMetrics, StakingMetrics — new struct families.
//!
//! All gauges/counters are lock-free `AtomicU64`. The render function
//! synthesises derived values (age, ratio) from base atomics at scrape time.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Monotonically increasing counter.
pub struct Counter(Arc<AtomicU64>);

impl Counter {
    pub fn new() -> Self { Counter(Arc::new(AtomicU64::new(0))) }
    pub fn inc(&self) { self.0.fetch_add(1, Ordering::Relaxed); }
    pub fn add(&self, n: u64) { self.0.fetch_add(n, Ordering::Relaxed); }
    pub fn get(&self) -> u64 { self.0.load(Ordering::Relaxed) }
}

impl Clone for Counter {
    fn clone(&self) -> Self { Counter(Arc::clone(&self.0)) }
}

/// A gauge (can go up or down).
pub struct Gauge(Arc<AtomicU64>);

impl Gauge {
    pub fn new() -> Self { Gauge(Arc::new(AtomicU64::new(0))) }
    pub fn set(&self, v: u64) { self.0.store(v, Ordering::Relaxed); }
    pub fn get(&self) -> u64 { self.0.load(Ordering::Relaxed) }
    pub fn inc(&self) { self.0.fetch_add(1, Ordering::Relaxed); }
    pub fn dec(&self) { self.0.fetch_sub(1, Ordering::Relaxed); }
}

impl Clone for Gauge {
    fn clone(&self) -> Self { Gauge(Arc::clone(&self.0)) }
}

/// Block-level metrics.
#[derive(Clone)]
pub struct BlockMetrics {
    pub committed_blocks: Counter,
    pub block_height: Gauge,
    pub gas_used_total: Counter,
    pub transactions_total: Counter,
    pub avg_block_time_ms: Gauge,
    pub reorgs: Counter,
    /// Unix milliseconds of the last commit. 0 if no block committed yet.
    pub last_committed_unix_ms: Gauge,
    /// State-root mismatch incidents (replay vs proposal). Should always be 0
    /// in production; non-zero indicates an execution-layer determinism bug.
    pub state_root_mismatch_total: Counter,
}

impl BlockMetrics {
    pub fn new() -> Self {
        BlockMetrics {
            committed_blocks: Counter::new(),
            block_height: Gauge::new(),
            gas_used_total: Counter::new(),
            transactions_total: Counter::new(),
            avg_block_time_ms: Gauge::new(),
            reorgs: Counter::new(),
            last_committed_unix_ms: Gauge::new(),
            state_root_mismatch_total: Counter::new(),
        }
    }

    pub fn on_block_committed(&self, height: u64, gas_used: u64, tx_count: u64) {
        self.committed_blocks.inc();
        self.block_height.set(height);
        self.gas_used_total.add(gas_used);
        self.transactions_total.add(tx_count);
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        self.last_committed_unix_ms.set(now_ms);
    }
}

/// Consensus metrics.
#[derive(Clone)]
pub struct ConsensusMetrics {
    pub rounds_total: Counter,
    pub timeouts_total: Counter,
    pub votes_cast: Counter,
    pub qc_formed: Counter,
    pub current_round: Gauge,
    pub current_epoch: Gauge,
    pub active_validators: Gauge,
    /// Quorum requirement (2f+1). Used to compute participation ratio.
    pub quorum_total: Gauge,
    /// Number of votes aggregated into the most recent QC.
    pub last_qc_size: Gauge,
    /// Equivocation evidence detected (Pass-10 wiring to slashing v2).
    pub equivocations_total: Counter,
}

impl ConsensusMetrics {
    pub fn new() -> Self {
        ConsensusMetrics {
            rounds_total: Counter::new(),
            timeouts_total: Counter::new(),
            votes_cast: Counter::new(),
            qc_formed: Counter::new(),
            current_round: Gauge::new(),
            current_epoch: Gauge::new(),
            active_validators: Gauge::new(),
            quorum_total: Gauge::new(),
            last_qc_size: Gauge::new(),
            equivocations_total: Counter::new(),
        }
    }
}

/// Mempool metrics.
#[derive(Clone)]
pub struct MempoolMetrics {
    pub pending_txs: Gauge,
    pub queued_txs: Gauge,
    pub txs_added: Counter,
    pub txs_evicted: Counter,
    pub txs_confirmed: Counter,
}

impl MempoolMetrics {
    pub fn new() -> Self {
        MempoolMetrics {
            pending_txs: Gauge::new(),
            queued_txs: Gauge::new(),
            txs_added: Counter::new(),
            txs_evicted: Counter::new(),
            txs_confirmed: Counter::new(),
        }
    }
}

/// Network / P2P metrics.
#[derive(Clone)]
pub struct NetworkMetrics {
    pub connected_peers: Gauge,
    pub bytes_sent: Counter,
    pub bytes_received: Counter,
    pub messages_sent: Counter,
    pub messages_received: Counter,
    pub dial_errors: Counter,
    /// Currently banned peers (persisted to banlist.json).
    pub banned_peers: Gauge,
}

impl NetworkMetrics {
    pub fn new() -> Self {
        NetworkMetrics {
            connected_peers: Gauge::new(),
            bytes_sent: Counter::new(),
            bytes_received: Counter::new(),
            messages_sent: Counter::new(),
            messages_received: Counter::new(),
            dial_errors: Counter::new(),
            banned_peers: Gauge::new(),
        }
    }
}

/// JSON-RPC server metrics (Pass-10).
#[derive(Clone)]
pub struct RpcMetrics {
    pub requests_total: Counter,
    pub errors_total: Counter,
    /// Cumulative request handling latency in microseconds (use with rate()).
    pub latency_us_sum: Counter,
    pub batched_requests_total: Counter,
    /// Rejections from batch gas budget guard (Pass-6 H-batch).
    pub batch_gas_budget_rejections_total: Counter,
    /// Rejections from `eth_call` / `eth_estimateGas` per-call gas cap.
    pub call_gas_cap_rejections_total: Counter,
    pub ws_connections: Gauge,
    pub ws_subscriptions: Gauge,
}

impl RpcMetrics {
    pub fn new() -> Self {
        RpcMetrics {
            requests_total: Counter::new(),
            errors_total: Counter::new(),
            latency_us_sum: Counter::new(),
            batched_requests_total: Counter::new(),
            batch_gas_budget_rejections_total: Counter::new(),
            call_gas_cap_rejections_total: Counter::new(),
            ws_connections: Gauge::new(),
            ws_subscriptions: Gauge::new(),
        }
    }
}

/// Bridge metrics (Pass-10).
#[derive(Clone)]
pub struct BridgeMetrics {
    pub deposits_total: Counter,
    pub withdrawals_total: Counter,
    pub pending_withdrawals: Gauge,
    /// Validator signatures collected on the most recent withdrawal proof.
    pub last_proof_sigs: Gauge,
    /// Threshold required (audit floor ≥2 — see Pass 4).
    pub bridge_threshold: Gauge,
    /// Bridge paused state (1 = paused, 0 = active).
    pub paused: Gauge,
}

impl BridgeMetrics {
    pub fn new() -> Self {
        BridgeMetrics {
            deposits_total: Counter::new(),
            withdrawals_total: Counter::new(),
            pending_withdrawals: Gauge::new(),
            last_proof_sigs: Gauge::new(),
            bridge_threshold: Gauge::new(),
            paused: Gauge::new(),
        }
    }
}

/// Staking + slashing metrics (Pass-10).
#[derive(Clone)]
pub struct StakingMetrics {
    pub total_stake_wei_lo: Gauge,           // u128 split: low 64 bits
    pub total_stake_wei_hi: Gauge,           // u128 split: high 64 bits
    pub active_validator_count: Gauge,
    pub jailed_validator_count: Gauge,
    pub slash_evidence_pending: Gauge,
    pub slash_evidence_confirmed_total: Counter,
    pub slash_evidence_overturned_total: Counter,
    /// Cumulative wei slashed (low 64 bits — for full value scrape low+hi).
    pub total_slashed_wei_lo: Counter,
    pub total_slashed_wei_hi: Counter,
}

impl StakingMetrics {
    pub fn new() -> Self {
        StakingMetrics {
            total_stake_wei_lo: Gauge::new(),
            total_stake_wei_hi: Gauge::new(),
            active_validator_count: Gauge::new(),
            jailed_validator_count: Gauge::new(),
            slash_evidence_pending: Gauge::new(),
            slash_evidence_confirmed_total: Counter::new(),
            slash_evidence_overturned_total: Counter::new(),
            total_slashed_wei_lo: Counter::new(),
            total_slashed_wei_hi: Counter::new(),
        }
    }

    /// Set the u128 stake value into two atomics (lo, hi).
    pub fn set_total_stake(&self, wei: u128) {
        self.total_stake_wei_lo.set(wei as u64);
        self.total_stake_wei_hi.set((wei >> 64) as u64);
    }
}

/// Aggregate of all metric families. Cheap to clone (Arc inside).
#[derive(Clone)]
pub struct Registry {
    pub blocks:    BlockMetrics,
    pub consensus: ConsensusMetrics,
    pub mempool:   MempoolMetrics,
    pub network:   NetworkMetrics,
    pub rpc:       RpcMetrics,
    pub bridge:    BridgeMetrics,
    pub staking:   StakingMetrics,
}

impl Registry {
    pub fn new() -> Self {
        Registry {
            blocks:    BlockMetrics::new(),
            consensus: ConsensusMetrics::new(),
            mempool:   MempoolMetrics::new(),
            network:   NetworkMetrics::new(),
            rpc:       RpcMetrics::new(),
            bridge:    BridgeMetrics::new(),
            staking:   StakingMetrics::new(),
        }
    }
}

impl Default for Registry { fn default() -> Self { Self::new() } }

/// Serialise all metrics in Prometheus text exposition format (v0.0.4).
///
/// Pass-10 fix: previous version emitted literal `\n` text (escape-in-string
/// regression) — Prometheus parsers reject the entire scrape. Rewritten with
/// `writeln!` so newlines are real `0x0A` bytes.
pub fn render_prometheus_full(reg: &Registry) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(4096);

    // ---- block ----------------------------------------------------------
    let _ = writeln!(s, "# HELP zbx_committed_blocks_total Total committed blocks");
    let _ = writeln!(s, "# TYPE zbx_committed_blocks_total counter");
    let _ = writeln!(s, "zbx_committed_blocks_total {}", reg.blocks.committed_blocks.get());

    let _ = writeln!(s, "# HELP zbx_block_height Current chain tip");
    let _ = writeln!(s, "# TYPE zbx_block_height gauge");
    let _ = writeln!(s, "zbx_block_height {}", reg.blocks.block_height.get());

    let _ = writeln!(s, "# HELP zbx_gas_used_total Cumulative gas used");
    let _ = writeln!(s, "# TYPE zbx_gas_used_total counter");
    let _ = writeln!(s, "zbx_gas_used_total {}", reg.blocks.gas_used_total.get());

    let _ = writeln!(s, "# HELP zbx_transactions_total Cumulative transactions");
    let _ = writeln!(s, "# TYPE zbx_transactions_total counter");
    let _ = writeln!(s, "zbx_transactions_total {}", reg.blocks.transactions_total.get());

    let _ = writeln!(s, "# HELP zbx_avg_block_time_ms Rolling block-time average");
    let _ = writeln!(s, "# TYPE zbx_avg_block_time_ms gauge");
    let _ = writeln!(s, "zbx_avg_block_time_ms {}", reg.blocks.avg_block_time_ms.get());

    let _ = writeln!(s, "# HELP zbx_reorgs_total Cumulative chain reorganisations");
    let _ = writeln!(s, "# TYPE zbx_reorgs_total counter");
    let _ = writeln!(s, "zbx_reorgs_total {}", reg.blocks.reorgs.get());

    let _ = writeln!(s, "# HELP zbx_state_root_mismatch_total Replay vs proposal divergences (must be 0)");
    let _ = writeln!(s, "# TYPE zbx_state_root_mismatch_total counter");
    let _ = writeln!(s, "zbx_state_root_mismatch_total {}", reg.blocks.state_root_mismatch_total.get());

    // Derived: age of last committed block in seconds (key liveness alarm).
    let last_ms = reg.blocks.last_committed_unix_ms.get();
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let age_secs = if last_ms == 0 || now_ms <= last_ms { 0 } else { (now_ms - last_ms) / 1000 };
    let _ = writeln!(s, "# HELP zbx_last_committed_age_seconds Seconds since last block commit (0 = none yet)");
    let _ = writeln!(s, "# TYPE zbx_last_committed_age_seconds gauge");
    let _ = writeln!(s, "zbx_last_committed_age_seconds {}", age_secs);

    // ---- consensus ------------------------------------------------------
    let _ = writeln!(s, "# HELP zbx_consensus_rounds_total Total consensus rounds");
    let _ = writeln!(s, "# TYPE zbx_consensus_rounds_total counter");
    let _ = writeln!(s, "zbx_consensus_rounds_total {}", reg.consensus.rounds_total.get());

    let _ = writeln!(s, "# HELP zbx_consensus_timeouts_total Total round timeouts");
    let _ = writeln!(s, "# TYPE zbx_consensus_timeouts_total counter");
    let _ = writeln!(s, "zbx_consensus_timeouts_total {}", reg.consensus.timeouts_total.get());

    let _ = writeln!(s, "# HELP zbx_consensus_votes_cast Total votes cast");
    let _ = writeln!(s, "# TYPE zbx_consensus_votes_cast counter");
    let _ = writeln!(s, "zbx_consensus_votes_cast {}", reg.consensus.votes_cast.get());

    let _ = writeln!(s, "# HELP zbx_consensus_qc_formed Total QCs formed");
    let _ = writeln!(s, "# TYPE zbx_consensus_qc_formed counter");
    let _ = writeln!(s, "zbx_consensus_qc_formed {}", reg.consensus.qc_formed.get());

    let _ = writeln!(s, "# HELP zbx_consensus_current_round Current HotStuff round");
    let _ = writeln!(s, "# TYPE zbx_consensus_current_round gauge");
    let _ = writeln!(s, "zbx_consensus_current_round {}", reg.consensus.current_round.get());

    let _ = writeln!(s, "# HELP zbx_consensus_current_epoch Current epoch");
    let _ = writeln!(s, "# TYPE zbx_consensus_current_epoch gauge");
    let _ = writeln!(s, "zbx_consensus_current_epoch {}", reg.consensus.current_epoch.get());

    let _ = writeln!(s, "# HELP zbx_active_validators Active validator count this epoch");
    let _ = writeln!(s, "# TYPE zbx_active_validators gauge");
    let _ = writeln!(s, "zbx_active_validators {}", reg.consensus.active_validators.get());

    let _ = writeln!(s, "# HELP zbx_consensus_quorum_total Quorum requirement (2f+1)");
    let _ = writeln!(s, "# TYPE zbx_consensus_quorum_total gauge");
    let _ = writeln!(s, "zbx_consensus_quorum_total {}", reg.consensus.quorum_total.get());

    let _ = writeln!(s, "# HELP zbx_consensus_last_qc_size Vote count in most-recent QC");
    let _ = writeln!(s, "# TYPE zbx_consensus_last_qc_size gauge");
    let _ = writeln!(s, "zbx_consensus_last_qc_size {}", reg.consensus.last_qc_size.get());

    // Derived: participation ratio = last_qc_size / quorum.
    let q = reg.consensus.quorum_total.get();
    let last = reg.consensus.last_qc_size.get();
    let ratio = if q == 0 { 0.0 } else { last as f64 / q as f64 };
    let _ = writeln!(s, "# HELP zbx_consensus_qc_participation_ratio last_qc_size / quorum (1.0 = full)");
    let _ = writeln!(s, "# TYPE zbx_consensus_qc_participation_ratio gauge");
    let _ = writeln!(s, "zbx_consensus_qc_participation_ratio {}", ratio);

    let _ = writeln!(s, "# HELP zbx_equivocations_total Equivocation evidence submitted to slashing v2");
    let _ = writeln!(s, "# TYPE zbx_equivocations_total counter");
    let _ = writeln!(s, "zbx_equivocations_total {}", reg.consensus.equivocations_total.get());

    // ---- mempool --------------------------------------------------------
    let _ = writeln!(s, "# HELP zbx_mempool_pending Pending transactions in mempool");
    let _ = writeln!(s, "# TYPE zbx_mempool_pending gauge");
    let _ = writeln!(s, "zbx_mempool_pending {}", reg.mempool.pending_txs.get());

    let _ = writeln!(s, "# HELP zbx_mempool_queued Queued transactions in mempool");
    let _ = writeln!(s, "# TYPE zbx_mempool_queued gauge");
    let _ = writeln!(s, "zbx_mempool_queued {}", reg.mempool.queued_txs.get());

    let _ = writeln!(s, "# HELP zbx_mempool_added_total Cumulative transactions added");
    let _ = writeln!(s, "# TYPE zbx_mempool_added_total counter");
    let _ = writeln!(s, "zbx_mempool_added_total {}", reg.mempool.txs_added.get());

    let _ = writeln!(s, "# HELP zbx_mempool_evicted_total Cumulative transactions evicted");
    let _ = writeln!(s, "# TYPE zbx_mempool_evicted_total counter");
    let _ = writeln!(s, "zbx_mempool_evicted_total {}", reg.mempool.txs_evicted.get());

    // ---- network --------------------------------------------------------
    let _ = writeln!(s, "# HELP zbx_network_peers_connected Connected peers");
    let _ = writeln!(s, "# TYPE zbx_network_peers_connected gauge");
    let _ = writeln!(s, "zbx_network_peers_connected {}", reg.network.connected_peers.get());

    let _ = writeln!(s, "# HELP zbx_network_peers_banned Currently banned peer count");
    let _ = writeln!(s, "# TYPE zbx_network_peers_banned gauge");
    let _ = writeln!(s, "zbx_network_peers_banned {}", reg.network.banned_peers.get());

    let _ = writeln!(s, "# HELP zbx_network_bytes_sent_total Total bytes sent");
    let _ = writeln!(s, "# TYPE zbx_network_bytes_sent_total counter");
    let _ = writeln!(s, "zbx_network_bytes_sent_total {}", reg.network.bytes_sent.get());

    let _ = writeln!(s, "# HELP zbx_network_bytes_received_total Total bytes received");
    let _ = writeln!(s, "# TYPE zbx_network_bytes_received_total counter");
    let _ = writeln!(s, "zbx_network_bytes_received_total {}", reg.network.bytes_received.get());

    let _ = writeln!(s, "# HELP zbx_network_dial_errors_total Outbound dial failures");
    let _ = writeln!(s, "# TYPE zbx_network_dial_errors_total counter");
    let _ = writeln!(s, "zbx_network_dial_errors_total {}", reg.network.dial_errors.get());

    // ---- rpc ------------------------------------------------------------
    let _ = writeln!(s, "# HELP zbx_rpc_requests_total Total JSON-RPC requests");
    let _ = writeln!(s, "# TYPE zbx_rpc_requests_total counter");
    let _ = writeln!(s, "zbx_rpc_requests_total {}", reg.rpc.requests_total.get());

    let _ = writeln!(s, "# HELP zbx_rpc_errors_total Total JSON-RPC error responses");
    let _ = writeln!(s, "# TYPE zbx_rpc_errors_total counter");
    let _ = writeln!(s, "zbx_rpc_errors_total {}", reg.rpc.errors_total.get());

    let _ = writeln!(s, "# HELP zbx_rpc_latency_us_sum Cumulative RPC handling time in microseconds");
    let _ = writeln!(s, "# TYPE zbx_rpc_latency_us_sum counter");
    let _ = writeln!(s, "zbx_rpc_latency_us_sum {}", reg.rpc.latency_us_sum.get());

    let _ = writeln!(s, "# HELP zbx_rpc_batch_gas_budget_rejections_total Batched-call gas-budget rejections (Pass-6)");
    let _ = writeln!(s, "# TYPE zbx_rpc_batch_gas_budget_rejections_total counter");
    let _ = writeln!(s, "zbx_rpc_batch_gas_budget_rejections_total {}", reg.rpc.batch_gas_budget_rejections_total.get());

    let _ = writeln!(s, "# HELP zbx_rpc_call_gas_cap_rejections_total eth_call/estimateGas per-call cap rejections (Pass-5 C8)");
    let _ = writeln!(s, "# TYPE zbx_rpc_call_gas_cap_rejections_total counter");
    let _ = writeln!(s, "zbx_rpc_call_gas_cap_rejections_total {}", reg.rpc.call_gas_cap_rejections_total.get());

    let _ = writeln!(s, "# HELP zbx_rpc_ws_connections Open WebSocket connections");
    let _ = writeln!(s, "# TYPE zbx_rpc_ws_connections gauge");
    let _ = writeln!(s, "zbx_rpc_ws_connections {}", reg.rpc.ws_connections.get());

    let _ = writeln!(s, "# HELP zbx_rpc_ws_subscriptions Active WS subscriptions");
    let _ = writeln!(s, "# TYPE zbx_rpc_ws_subscriptions gauge");
    let _ = writeln!(s, "zbx_rpc_ws_subscriptions {}", reg.rpc.ws_subscriptions.get());

    // ---- bridge ---------------------------------------------------------
    let _ = writeln!(s, "# HELP zbx_bridge_deposits_total Cross-chain deposits inbound");
    let _ = writeln!(s, "# TYPE zbx_bridge_deposits_total counter");
    let _ = writeln!(s, "zbx_bridge_deposits_total {}", reg.bridge.deposits_total.get());

    let _ = writeln!(s, "# HELP zbx_bridge_withdrawals_total Cross-chain withdrawals outbound");
    let _ = writeln!(s, "# TYPE zbx_bridge_withdrawals_total counter");
    let _ = writeln!(s, "zbx_bridge_withdrawals_total {}", reg.bridge.withdrawals_total.get());

    let _ = writeln!(s, "# HELP zbx_bridge_pending_withdrawals Pending withdrawal proofs awaiting quorum");
    let _ = writeln!(s, "# TYPE zbx_bridge_pending_withdrawals gauge");
    let _ = writeln!(s, "zbx_bridge_pending_withdrawals {}", reg.bridge.pending_withdrawals.get());

    let _ = writeln!(s, "# HELP zbx_bridge_threshold Required validator signatures (≥2)");
    let _ = writeln!(s, "# TYPE zbx_bridge_threshold gauge");
    let _ = writeln!(s, "zbx_bridge_threshold {}", reg.bridge.bridge_threshold.get());

    let _ = writeln!(s, "# HELP zbx_bridge_paused Bridge circuit-breaker state (1 = paused)");
    let _ = writeln!(s, "# TYPE zbx_bridge_paused gauge");
    let _ = writeln!(s, "zbx_bridge_paused {}", reg.bridge.paused.get());

    // ---- staking --------------------------------------------------------
    let _ = writeln!(s, "# HELP zbx_staking_active_validators Active validator count");
    let _ = writeln!(s, "# TYPE zbx_staking_active_validators gauge");
    let _ = writeln!(s, "zbx_staking_active_validators {}", reg.staking.active_validator_count.get());

    let _ = writeln!(s, "# HELP zbx_staking_jailed_validators Jailed validator count");
    let _ = writeln!(s, "# TYPE zbx_staking_jailed_validators gauge");
    let _ = writeln!(s, "zbx_staking_jailed_validators {}", reg.staking.jailed_validator_count.get());

    let _ = writeln!(s, "# HELP zbx_staking_total_stake_wei_lo Total bonded stake (low 64 bits of u128)");
    let _ = writeln!(s, "# TYPE zbx_staking_total_stake_wei_lo gauge");
    let _ = writeln!(s, "zbx_staking_total_stake_wei_lo {}", reg.staking.total_stake_wei_lo.get());

    let _ = writeln!(s, "# HELP zbx_staking_total_stake_wei_hi Total bonded stake (high 64 bits of u128)");
    let _ = writeln!(s, "# TYPE zbx_staking_total_stake_wei_hi gauge");
    let _ = writeln!(s, "zbx_staking_total_stake_wei_hi {}", reg.staking.total_stake_wei_hi.get());

    let _ = writeln!(s, "# HELP zbx_slash_evidence_pending Pending slash evidence in registry");
    let _ = writeln!(s, "# TYPE zbx_slash_evidence_pending gauge");
    let _ = writeln!(s, "zbx_slash_evidence_pending {}", reg.staking.slash_evidence_pending.get());

    let _ = writeln!(s, "# HELP zbx_slash_evidence_confirmed_total Confirmed slash records");
    let _ = writeln!(s, "# TYPE zbx_slash_evidence_confirmed_total counter");
    let _ = writeln!(s, "zbx_slash_evidence_confirmed_total {}", reg.staking.slash_evidence_confirmed_total.get());

    let _ = writeln!(s, "# HELP zbx_slash_evidence_overturned_total Slashes overturned on appeal");
    let _ = writeln!(s, "# TYPE zbx_slash_evidence_overturned_total counter");
    let _ = writeln!(s, "zbx_slash_evidence_overturned_total {}", reg.staking.slash_evidence_overturned_total.get());

    s
}

/// Backward-compatible wrapper retained for callers that pre-date the
/// `Registry` aggregate (e.g. legacy MetricsServer plumbing). Builds a
/// minimal registry from the four per-family handles passed in.
pub fn render_prometheus(
    blocks: &BlockMetrics,
    consensus: &ConsensusMetrics,
    mempool: &MempoolMetrics,
    network: &NetworkMetrics,
) -> String {
    let reg = Registry {
        blocks:    blocks.clone(),
        consensus: consensus.clone(),
        mempool:   mempool.clone(),
        network:   network.clone(),
        rpc:       RpcMetrics::new(),
        bridge:    BridgeMetrics::new(),
        staking:   StakingMetrics::new(),
    };
    render_prometheus_full(&reg)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pass-10 regression: render output must contain real newlines, not
    /// the literal two-character sequence `\n`.
    #[test]
    fn render_emits_real_newlines() {
        let reg = Registry::new();
        reg.blocks.committed_blocks.add(7);
        let out = render_prometheus_full(&reg);
        // Should have many real newlines.
        let nl_count = out.bytes().filter(|b| *b == b'\n').count();
        assert!(nl_count > 30, "expected >30 real newlines, got {}", nl_count);
        // Should NOT contain the literal escape sequence `\n` as text.
        assert!(!out.contains("\\n"), "render output still emits literal '\\n' escape");
        // Spot-check a counter line is present and well-formed.
        assert!(out.contains("zbx_committed_blocks_total 7\n"));
    }

    /// Every metric line must be a valid prom-text line: starts with `#` or
    /// `<name> <number>`. Helps catch missing values / format breakage.
    #[test]
    fn render_lines_are_well_formed() {
        let reg = Registry::new();
        let out = render_prometheus_full(&reg);
        for (i, line) in out.lines().enumerate() {
            if line.is_empty() { continue; }
            if line.starts_with('#') { continue; }
            // Expect "<name> <value>"
            let mut parts = line.splitn(2, ' ');
            let name = parts.next().unwrap_or("");
            let val  = parts.next().unwrap_or("");
            assert!(name.starts_with("zbx_"),
                "line {} does not start with zbx_: {:?}", i, line);
            assert!(!val.is_empty(),
                "line {} missing value: {:?}", i, line);
        }
    }

    #[test]
    fn last_committed_age_is_zero_when_never_committed() {
        let reg = Registry::new();
        let out = render_prometheus_full(&reg);
        assert!(out.contains("zbx_last_committed_age_seconds 0\n"));
    }

    #[test]
    fn participation_ratio_handles_zero_quorum() {
        let reg = Registry::new();
        // quorum=0 must NOT panic / divide-by-zero — should emit 0.
        let out = render_prometheus_full(&reg);
        assert!(out.contains("zbx_consensus_qc_participation_ratio 0\n"));
    }

    #[test]
    fn participation_ratio_two_thirds() {
        let reg = Registry::new();
        reg.consensus.quorum_total.set(3);
        reg.consensus.last_qc_size.set(2);
        let out = render_prometheus_full(&reg);
        assert!(out.contains("zbx_consensus_qc_participation_ratio 0.6666666666666666\n")
             || out.contains("zbx_consensus_qc_participation_ratio 0.6666666666666667\n"),
             "unexpected ratio in output:\n{}", out);
    }

    #[test]
    fn staking_u128_split_roundtrips() {
        let reg = Registry::new();
        let v: u128 = (1u128 << 70) | 12345; // crosses 64-bit boundary
        reg.staking.set_total_stake(v);
        let lo = reg.staking.total_stake_wei_lo.get() as u128;
        let hi = (reg.staking.total_stake_wei_hi.get() as u128) << 64;
        assert_eq!(lo | hi, v);
    }

    #[test]
    fn legacy_render_function_still_works() {
        // 4-arg back-compat wrapper.
        let b = BlockMetrics::new();
        let c = ConsensusMetrics::new();
        let m = MempoolMetrics::new();
        let n = NetworkMetrics::new();
        b.committed_blocks.add(42);
        let out = render_prometheus(&b, &c, &m, &n);
        assert!(out.contains("zbx_committed_blocks_total 42\n"));
        assert!(!out.contains("\\n"));
    }
}
