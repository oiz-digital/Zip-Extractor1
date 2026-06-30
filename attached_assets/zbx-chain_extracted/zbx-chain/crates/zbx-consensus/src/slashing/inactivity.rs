//! Inactivity penalties and slashing window.
//!
//! ## Inactivity Leak
//!   If the chain fails to finalize for > 4 epochs, validators that are
//!   offline (not attesting) begin to lose stake via inactivity leak.
//!   This ensures the chain can eventually finalize even with <2/3 online.
//!
//!   Inactivity score increases by 4 for each missed epoch.
//!   Inactivity score decreases by 1 for each participated epoch.
//!   Penalty = inactivity_score * INACTIVITY_PENALTY_QUOTIENT / total_stake
//!
//!   INACTIVITY_PENALTY_QUOTIENT = 2^24 = 16,777,216
//!   (designed to bleed 50% of stake in ~21 days of total inactivity)
//!
//! ## Slashing window
//!   Slashing evidence must be submitted within SLASH_WINDOW epochs.
//!   After SLASH_WINDOW, evidence is too old and ignored.
//!   SLASH_WINDOW = 8192 epochs (~36 days at 6.4 min/epoch)
//!
//! ## Whistleblower / Grievance reward
//!   The node that submits valid slashing evidence receives a reward:
//!   WHISTLEBLOWER_REWARD = slashed_amount / WHISTLEBLOWER_REWARD_QUOTIENT
//!   WHISTLEBLOWER_REWARD_QUOTIENT = 512
//!   Remaining slashed amount goes to the burn address.

// ── Inactivity penalty constants ──────────────────────────────────────────────

/// Number of epochs without finality before inactivity leak starts.
pub const INACTIVITY_LEAK_EPOCH_THRESHOLD: u64 = 4;

/// Score increase per missed epoch during inactivity leak.
pub const INACTIVITY_SCORE_BIAS: u64 = 4;

/// Score decrease per participated epoch.
pub const INACTIVITY_SCORE_RECOVERY_RATE: u64 = 1;

/// Penalty quotient: penalty = stake * inactivity_score / QUOTIENT
pub const INACTIVITY_PENALTY_QUOTIENT: u64 = 1 << 24; // 16,777,216

/// Maximum inactivity score (caps exponential growth).
pub const MAX_INACTIVITY_SCORE: u64 = 1 << 32;

// ── Inactivity tracking ───────────────────────────────────────────────────────

/// Per-validator inactivity tracking state.
#[derive(Debug, Clone, Default)]
pub struct InactivityState {
    /// Current inactivity score (higher = larger penalty).
    pub score:          u64,
    /// Epochs since last participation.
    pub missed_epochs:  u64,
    /// Total inactivity penalty applied so far (wei).
    pub total_penalty:  u128,
}

/// InactivityPenalty result for one epoch.
#[derive(Debug)]
pub struct InactivityPenalty {
    pub validator:   [u8; 20],
    pub penalty_wei: u128,
    pub new_score:   u64,
}

/// Calculate inactivity penalties for all validators after an epoch.
///
/// Called at epoch N+1 if epoch N was NOT finalized (chain is leaking).
pub fn compute_inactivity_penalties(
    validators:          &[([u8; 20], u128)],  // (address, stake)
    inactivity_states:   &mut std::collections::HashMap<[u8; 20], InactivityState>,
    participated:        &std::collections::HashSet<[u8; 20]>,
    epochs_since_finality: u64,
) -> Vec<InactivityPenalty> {
    let mut penalties = Vec::new();

    // Only apply inactivity leak if chain hasn't finalized in N epochs
    let is_leaking = epochs_since_finality > INACTIVITY_LEAK_EPOCH_THRESHOLD;

    for (addr, stake) in validators {
        let state = inactivity_states.entry(*addr).or_default();
        let did_participate = participated.contains(addr);

        // Update inactivity score
        if did_participate {
            // Recovering: decrease score
            state.score = state.score.saturating_sub(INACTIVITY_SCORE_RECOVERY_RATE);
            state.missed_epochs = 0;
        } else {
            // Missing: increase score
            state.missed_epochs += 1;
            if is_leaking {
                state.score = (state.score + INACTIVITY_SCORE_BIAS).min(MAX_INACTIVITY_SCORE);
            }
        }

        // Compute penalty (only if leaking AND validator inactive)
        if is_leaking && !did_participate && state.score > 0 {
            // CSN-01 FIX (MEDIUM): previous code cast `stake: u128` → `u64`
            // before multiplying, silently truncating any stake > u64::MAX
            // (~18.4 ZBX in wei). A validator with, say, 100 ZBX staked
            // would have their stake mis-read as ~100 ZBX mod 2^64, producing
            // an arbitrarily wrong (and potentially zero) penalty. All
            // arithmetic now stays in u128 so the full stake precision is
            // preserved. `INACTIVITY_PENALTY_QUOTIENT` is cast to u128 for
            // the division; it fits easily (value = 2^24 = 16,777,216).
            let penalty = stake
                .saturating_mul(state.score as u128)
                / INACTIVITY_PENALTY_QUOTIENT as u128;
            state.total_penalty = state.total_penalty.saturating_add(penalty);
            penalties.push(InactivityPenalty {
                validator:   *addr,
                penalty_wei: penalty,
                new_score:   state.score,
            });
        }
    }
    penalties
}

// ── Slashing window ───────────────────────────────────────────────────────────

/// Maximum epoch age of slashing evidence (after this, evidence is too old).
pub const SLASH_WINDOW: u64 = 8192; // ~36 days

/// Check if slashing evidence is within the valid slashing window.
///
/// Evidence is valid if: current_epoch - evidence_epoch <= SLASH_WINDOW
pub fn is_within_slashing_window(current_epoch: u64, evidence_epoch: u64) -> bool {
    current_epoch.saturating_sub(evidence_epoch) <= SLASH_WINDOW
}

/// Record of slashing evidence with timestamp for window enforcement.
#[derive(Debug, Clone)]
pub struct SlashingRecord {
    pub validator:      [u8; 20],
    pub evidence_epoch: u64,
    pub slashed_at:     u64, // epoch when slashing was applied
    pub amount:         u128,
    pub evidence_type:  SlashEvidenceType,
}

#[derive(Debug, Clone)]
pub enum SlashEvidenceType {
    DoubleVote    { vote_a: [u8; 32], vote_b: [u8; 32] },
    DoublePropose { block_a: [u8; 32], block_b: [u8; 32] },
    SurroundVote  { outer_source: u64, outer_target: u64, inner_source: u64, inner_target: u64 },
}

// ── Whistleblower / Grievance reward ─────────────────────────────────────────

/// Quotient for whistleblower reward calculation.
/// Reward = slashed_amount / WHISTLEBLOWER_REWARD_QUOTIENT
pub const WHISTLEBLOWER_REWARD_QUOTIENT: u128 = 512;

/// Minimum whistleblower reward (in ZBX wei).
pub const MIN_WHISTLEBLOWER_REWARD: u128 = 1_000_000_000_000_000_000; // 1 ZBX

/// Whistleblower grievance record.
/// The reporter (whistleblower) who submits valid slashing evidence
/// receives a portion of the slashed validator's stake.
#[derive(Debug, Clone)]
pub struct WhistleblowerGrievance {
    /// Address of the reporter who submitted the evidence
    pub reporter:         [u8; 20],
    /// The slashed validator
    pub slashed:          [u8; 20],
    /// Evidence submitted
    pub evidence_type:    SlashEvidenceType,
    /// Total amount slashed from validator
    pub slashed_amount:   u128,
    /// Reporter reward (slashed_amount / WHISTLEBLOWER_REWARD_QUOTIENT)
    pub reporter_reward:  u128,
    /// Amount burned (slashed_amount - reporter_reward)
    pub burned_amount:    u128,
    /// Epoch of the infraction
    pub infraction_epoch: u64,
}

/// Compute whistleblower reward and burned amount for a slash.
pub fn compute_whistleblower_reward(slashed_amount: u128) -> (u128, u128) {
    let reward = (slashed_amount / WHISTLEBLOWER_REWARD_QUOTIENT).max(MIN_WHISTLEBLOWER_REWARD);
    let reward = reward.min(slashed_amount); // can't reward more than slashed
    let burned = slashed_amount.saturating_sub(reward);
    (reward, burned) // (reporter_reward, burned)
}

/// Process a slash with whistleblower grievance.
pub fn process_slash_with_grievance(
    validator:         [u8; 20],
    reporter:          [u8; 20],
    validator_stake:   u128,
    slash_percentage:  u8,  // e.g. 5 = 5%
    evidence:          SlashEvidenceType,
    current_epoch:     u64,
    evidence_epoch:    u64,
) -> Result<WhistleblowerGrievance, SlashError> {
    // Check slashing window
    if !is_within_slashing_window(current_epoch, evidence_epoch) {
        return Err(SlashError::EvidenceTooOld {
            evidence_epoch,
            window_end: evidence_epoch + SLASH_WINDOW,
        });
    }

    let slashed_amount = validator_stake
        .saturating_mul(slash_percentage as u128)
        / 100;

    let (reporter_reward, burned_amount) = compute_whistleblower_reward(slashed_amount);

    Ok(WhistleblowerGrievance {
        reporter,
        slashed:          validator,
        evidence_type:    evidence,
        slashed_amount,
        reporter_reward,
        burned_amount,
        infraction_epoch: evidence_epoch,
    })
}

#[derive(Debug)]
pub enum SlashError {
    EvidenceTooOld { evidence_epoch: u64, window_end: u64 },
    AlreadySlashed,
    ValidatorNotFound,
    InvalidEvidence,
}