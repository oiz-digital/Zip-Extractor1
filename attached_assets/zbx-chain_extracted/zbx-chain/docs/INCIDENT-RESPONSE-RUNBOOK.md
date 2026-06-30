# ZBX Chain — Incident Response Runbook

**Status**: v0.1 (template — must be customized before mainnet)
**Last updated**: 2026-05-09
**Owner**: SRE / On-call rotation
**Audience**: validator operators, RPC operators, bridge operators, core team on-call

> This runbook is the **operational** counterpart to the security passes.
> Code-level fixes prevent classes of bugs; this document tells humans
> what to do when something goes wrong at 3 AM.
>
> Mainnet is NOT ready until this runbook has been (a) reviewed by the
> SRE lead, (b) dry-run on a throwaway chain at least once per scenario
> below, and (c) wired to actual pager / Slack / status-page integrations.

---

## 1. On-call structure

- **Primary** (P0): one engineer, 1-week rotation, < 5 min response SLA for SEV-1.
- **Secondary** (P1): backup, escalates if primary unreachable in 15 min.
- **Incident Commander** (IC): for SEV-1 events, takes over comms and decisions.
- **Comms lead**: drafts status-page posts and external comms.

Rotation calendar: TBD (recommend PagerDuty / Opsgenie).

---

## 2. Severity classification

| Severity | Definition | Examples | SLA |
|----------|-----------|----------|-----|
| **SEV-1** | Chain liveness halted OR funds at risk | Block production stopped > 30 s, validator BFT participation < 2/3, bridge anomaly, state-root divergence detected | 5 min ack, 15 min IC, 1 h public status |
| **SEV-2** | Degraded but operational | RPC error rate > 5%, mempool back-pressure spike, single-validator slash, peer count drop > 50% | 15 min ack, 4 h fix |
| **SEV-3** | Non-urgent issue | Metrics gap, doc bug, single-RPC-method failure | Next business day |

---

## 3. Pager triggers (Prometheus → Alertmanager)

The `zbx-metrics` crate (port 9000) currently exports the **EXISTS** metrics
listed below. The **MISSING** rows describe gauges/counters that operators
need but that are NOT yet wired into the metrics export — those must be
added before mainnet (tracked as Pass-10+ engineering work). Don't paste
the MISSING names into Prometheus rules expecting them to fire today.

### Existing — wire into Alertmanager today

| Alert | Metric | Threshold | Severity |
|-------|--------|-----------|----------|
| Block height stalled | `zbx_block_height` (no change for > 30 s) | unchanged for 30 s | SEV-1 |
| Active validators dropped | `zbx_active_validators` | < 2/3 of expected for 2 min | SEV-1 |
| Consensus timeouts spiking | `zbx_consensus_timeouts_total` (rate) | > expected × 5 | SEV-2 |
| Reorg | `zbx_reorgs` (rate) | > 0 | SEV-1 |
| Disk free | `node_filesystem_avail_bytes{mountpoint=~"/var/lib/zbx.*"}` | < 20 GiB | SEV-2 |
| Cert expiry | nginx TLS cert age | < 14 days | SEV-3 |

(Existing metrics list inferred from `crates/zbx-metrics/src/counters.rs` —
verify against the live `/metrics` endpoint before deploying alert rules.)

### Missing — must be implemented before mainnet (Pass-10+)

| Desired alert | Suggested metric name | Threshold | Severity | Status |
|---------------|-----------------------|-----------|----------|--------|
| Block production halted (precise) | `zbx_consensus_last_committed_age_seconds` (gauge) | > 30 | SEV-1 | TODO |
| BFT participation low | `zbx_consensus_qc_participation_ratio` (gauge) | < 0.66 for 2 min | SEV-1 | TODO |
| State root divergence | `zbx_state_root_mismatch_total` (counter, rate) | > 0 | SEV-1 | TODO |
| Bridge outflow spike | `zbx_bridge_outflow_wei_per_minute` (gauge) | > 7-day-mean × 10 | SEV-1 | TODO |
| RPC 5xx rate | `zbx_rpc_response_status_total{status=~"5.."}` (counter, rate) | > 5% over 5 min | SEV-2 | TODO |
| Mempool fullness | `zbx_mempool_pending_size` / configured cap | > 0.9 for 5 min | SEV-2 | TODO |
| Peer count drop | `zbx_p2p_connected_peers` (gauge) | < max_peers / 4 for 10 min | SEV-2 | TODO |
| Validator slash event | `zbx_staking_slash_events_total` (counter, rate) | > 0 | SEV-2 | TODO |

> Implementing these is straightforward (add fields to the relevant
> `Metrics` structs in `zbx-metrics`, increment from the call sites, and
> render in the `/metrics` Prometheus-text exporter), but it is a real
> engineering item that has not been done yet.

---

## 4. Scenario playbooks

### 4.1 SEV-1: Block production halted (> 30 s, no new blocks)

1. **Acknowledge** page within 5 min. Open `#incident-active` channel.
2. **Confirm** by querying `eth_blockNumber` against ≥ 3 of your own RPC nodes.
3. **Check validator participation**:
   ```
   curl -s http://<rpc>/ -d '{"jsonrpc":"2.0","method":"zbx_consensusStatus","params":[],"id":1}'
   ```
4. **If participation < 2/3** → likely network partition or > 1/3 validator outage:
   - Page secondary validators in the active set
   - Check geographic / cloud-provider correlation (was AWS us-east-1 down?)
   - Do **NOT** restart the leader yet — let view-change run
5. **If participation ≥ 2/3 but no proposal** → leader stuck. Acceptable actions:
   - Wait one full view-change timeout (default 12 s) before intervening
   - If still stuck after 2 view-changes, gracefully restart the proposer node
6. **Comms**: post initial status-page entry within 1 h ("investigating", no speculation).
7. **Post-mortem**: required within 7 days, public within 14.

### 4.2 SEV-1: Bridge anomaly (large outflow / fast-burn rate)

1. **Acknowledge** within 5 min.
2. **Pause the bridge IMMEDIATELY** (Pass-1 added `Pausable` to `contracts/ZbxBridge.sol`):
   ```
   cast send <ZbxBridge> "pause()" --rpc-url $RPC --account guardian-multisig
   ```
   Requires guardian multisig (offline coordination). Verify the exact ABI
   against the deployed `ZbxBridge.sol` — the function name and access
   modifier (`onlyOwner` / `onlyGuardian` / role-gated) MUST be confirmed
   on the in-production contract before relying on this command.
3. **Snapshot** the current state of the bridge contract + locked balances.
4. **Investigate** before unpausing. Possible causes:
   - Compromised relayer signing key → rotate the affected key, do not unpause until 5-of-9 (or current threshold) clean signers confirmed
   - Smart-contract bug → coordinate with auditors, prepare hot-fix proposal
   - Oracle manipulation → check oracle freshness. Pass-3 added a
     `MAX_ORACLE_DELAY = 1h` `latestRoundData` staleness check to the
     trading-layer Solidity contracts (`ZbxPerpetuals` / `ZbxSpotOrderBook` /
     `ZbxOptions` / `ZbxDatedFutures`). The bridge itself does not currently
     consume an oracle — if it does in the future, mirror the same staleness
     guard.
5. **Do NOT unpause** without IC sign-off and at least 24 h forensic review.

### 4.3 SEV-1: State root divergence detected

1. This is a chain-fork condition. Treat as catastrophic.
2. **STOP** advancing your local node (kill `zbx-node`).
3. Snapshot the data dir: `tar -czf /backups/zbx-divergence-$(date +%s).tar.gz <data_dir>`
4. Compare your block-N state root against ≥ 2 other independent operators.
5. Whichever side is in the minority must roll back to the divergence height
   and re-sync from majority peers.
6. Post-mortem MANDATORY — divergence is a consensus / determinism bug,
   either in our code or in adversarial input. Triage to core team within 2 h.

### 4.4 SEV-1: Validator BLS key suspected compromised

1. **Immediately stop the validator process** on the compromised host.
2. **Do NOT** restart with the same key on a different host — that triggers
   the equivocation guard (Pass-5 H3) but the consensus-side detector and
   ZEP-023 slashing v2 will burn the bond regardless.
3. Use the validator-rotate procedure (TBD: link to `docs/VALIDATOR-KEY-ROTATION.md`)
   to graduate a fresh BLS key signed by your operator key.
4. Wait one epoch for the active set to update before the new key votes.

### 4.5 SEV-2: RPC 5xx rate spike

1. **Check rate-limiter** rejections (HTTP 429) vs actual 500s.
2. **Check per-method breakdown** in metrics — usually `eth_call` or
   `eth_estimateGas` from a runaway dApp.
3. Recall the RPC limits enforced in code (Pass-5 C8 + Pass-6 H-batch):
   - `RPC_GAS_CAP = 50M` per call
   - `RPC_BATCH_GAS_BUDGET = 100M` per JSON-RPC batch
   - `RPC_MAX_CALLDATA = 128 KiB`
4. If a single client is abusing — block at nginx layer (operator-side).
5. If a contract is genuinely heavy — file ticket to lower the gas cap with the dApp team.

### 4.6 SEV-2: Mempool full

1. Check `zbx_mempool_pending_size` and `zbx_mempool_max_slots_per_sender`.
2. Recall (Pass-4 R2): `max_slots_per_sender = 64` with cumulative balance reservation.
3. If a single sender is at cap — that's working as designed.
4. If the global cap is hit — likely a fee market issue. Check `eth_gasPrice` is rising.

---

## 5. Snapshot / restore

**Pre-mainnet checklist**:

- [ ] `scripts/snapshot.sh` tested on a real-volume data dir (≥ 10 GiB).
- [ ] Restore-from-snapshot tested end-to-end on a fresh VPS.
- [ ] Snapshot cadence agreed: hourly incremental, daily full, off-site retention 30 days.
- [ ] Genesis file backed up to ≥ 3 independent locations (S3, IPFS, paper).

---

## 6. Comms templates

### Initial status-page post (within 1 h)

> **Investigating** — We are aware of [symptom] on ZBX [mainnet/testnet]
> starting at [UTC time]. The team is investigating and will post an update
> within 1 hour.

### Resolved status-page post

> **Resolved** — The [symptom] incident on [date] was caused by [root cause].
> [Action taken]. Service has been fully restored as of [UTC time].
> Full post-mortem will be published at [URL] within 14 days.

### Bridge-pause public notice

> **Bridge paused** — As a precaution, ZBX Bridge has been paused while
> we investigate [symptom]. **No user funds are at risk.** We will provide
> updates every 2 hours until the bridge is reopened. Expected resolution: [estimate].

---

## 7. Post-mortem template

Every SEV-1 requires a post-mortem within 7 days, public within 14:

```
# ZBX Incident Post-Mortem — YYYY-MM-DD

## Summary
[1-paragraph executive summary]

## Timeline (UTC)
- HH:MM — first symptom observed
- HH:MM — paged
- HH:MM — IC assigned
- HH:MM — root cause identified
- HH:MM — mitigation applied
- HH:MM — service restored
- HH:MM — all-clear

## Root cause
[Technical detail — what code/config/operational issue caused this]

## Detection
[How was it detected? Was the alert appropriate? Time-to-detect?]

## Mitigation
[What was done to stop the bleeding immediately]

## Permanent fix
[What code/config/process change prevents recurrence]
[Link to PR / commit / config change]

## Lessons learned
- What went well
- What went poorly
- What we got lucky on

## Action items
| # | Item | Owner | Due |
|---|------|-------|-----|
| 1 | ... | ... | ... |
```

---

## 8. Escalation contacts (TBD before mainnet)

| Role | Person | Pager | Slack | Email |
|------|--------|-------|-------|-------|
| On-call primary | TBD | TBD | TBD | TBD |
| On-call secondary | TBD | TBD | TBD | TBD |
| Incident commander pool | TBD | TBD | TBD | TBD |
| Bridge guardian multisig signers | TBD (≥ 5 of 9) | TBD | TBD | TBD |
| External audit firm contacts | TBD | TBD | TBD | TBD |
| Comms lead | TBD | TBD | TBD | TBD |
| Legal counsel | TBD | TBD | TBD | TBD |

---

## 9. What this runbook does NOT cover (yet)

These need to be written before mainnet:

- **Genesis ceremony procedure** — air-gapped key generation, video record,
  Shamir sharding. (Separate doc: `docs/GENESIS-CEREMONY.md` — TBD)
- **Validator key rotation procedure** — graduated rotation,
  HSM ↔ HSM transfer. (Separate doc: `docs/VALIDATOR-KEY-ROTATION.md` — TBD)
- **Hard fork / network upgrade procedure** — coordination across all
  validators, fork-choice rule, fallback if upgrade fails. (Separate doc — TBD)
- **Subpoena / legal-process response** — coordinate with counsel. (Separate doc — TBD)

---

## 10. Sign-off requirements before mainnet

- [ ] SRE lead has reviewed and signed off on this runbook.
- [ ] Each scenario in §4 has been **dry-run on a throwaway testnet** at least once.
- [ ] All TBDs in §8 are filled in with real names + pager numbers.
- [ ] PagerDuty / Opsgenie integration tested end-to-end (test page received).
- [ ] Status page (status.zbx.io) live and tested.
- [ ] Bridge guardian multisig signers have been drilled on §4.2.
- [ ] Snapshot/restore §5 has been verified on real-volume data.
