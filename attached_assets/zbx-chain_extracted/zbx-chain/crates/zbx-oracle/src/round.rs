//! Oracle round management — tracks open/closed rounds per feed.
//!
//! Each price update is a "round" (Chainlink terminology).
//! Rounds are numbered incrementally. Each round:
//!   1. Opens: reporters submit prices
//!   2. Closes: aggregation produces a result
//!   3. Result committed on-chain via ZBX transaction

use crate::{
    feed::{FeedId, PriceFeed, Price},
    reporter::PriceReport,
    aggregator::{OracleAggregator, AggregateResult},
    error::OracleError,
};
use serde::{Serialize, Deserialize};
use std::collections::HashMap;

/// Round identifier (incrementing u64, per feed).
pub type RoundId = u64;

/// State of a single oracle round.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RoundState {
    /// Collecting reports from reporters
    Open { opened_at: u64 },
    /// Enough reports collected — aggregated
    Closed { result: AggregateResult },
    /// Not enough reporters within timeout — round failed
    Failed { reason: String },
}

/// One oracle round for one price feed.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OracleRound {
    pub round_id:   RoundId,
    pub feed_id:    FeedId,
    pub state:      RoundState,
    pub reports:    Vec<PriceReport>,
    pub max_age_secs: u64,
}

impl OracleRound {
    /// Start a new open round.
    pub fn open(round_id: RoundId, feed_id: FeedId, now: u64, max_age_secs: u64) -> Self {
        tracing::debug!(round = round_id, feed = %feed_id, "Oracle round opened");
        Self {
            round_id,
            feed_id,
            state: RoundState::Open { opened_at: now },
            reports: Vec::new(),
            max_age_secs,
        }
    }

    /// Add a price report from a reporter.
    pub fn add_report(
        &mut self,
        report:    PriceReport,
        now:       u64,
        whitelist: &[[u8; 20]],
    ) -> Result<(), OracleError> {
        // Must be open
        if !matches!(self.state, RoundState::Open { .. }) {
            return Err(OracleError::RoundNotOpen(self.round_id));
        }

        // Reporter must be whitelisted
        if !whitelist.contains(&report.reporter) {
            return Err(OracleError::UnauthorizedReporter(report.reporter));
        }

        // No duplicate from same reporter
        if self.reports.iter().any(|r| r.reporter == report.reporter) {
            return Err(OracleError::DuplicateReport(report.reporter));
        }

        // Signature valid
        if !report.verify_sig() {
            return Err(OracleError::InvalidSignature);
        }

        // Not expired
        if report.is_expired(now, self.max_age_secs) {
            return Err(OracleError::ReportExpired);
        }

        self.reports.push(report);
        tracing::debug!(round = self.round_id, reporters = self.reports.len(), "Report added");
        Ok(())
    }

    /// Close this round — aggregate collected reports.
    pub fn close(
        &mut self,
        aggregator: &OracleAggregator,
        now:        u64,
        min_reporters: u32,
    ) -> Result<&AggregateResult, OracleError> {
        if !matches!(self.state, RoundState::Open { .. }) {
            return Err(OracleError::RoundNotOpen(self.round_id));
        }

        match aggregator.aggregate(&self.reports, now) {
            Ok(result) => {
                tracing::info!(
                    round = self.round_id,
                    feed  = %self.feed_id,
                    price = %result.price,
                    reporters = result.reporter_count,
                    "Oracle round closed ✅"
                );
                self.state = RoundState::Closed { result };
                if let RoundState::Closed { result } = &self.state {
                    Ok(result)
                } else { unreachable!() }
            }
            Err(e) => {
                let msg = e.to_string();
                self.state = RoundState::Failed { reason: msg.clone() };
                tracing::warn!(round = self.round_id, reason = msg, "Oracle round failed");
                Err(e)
            }
        }
    }

    /// The aggregated price (if closed).
    pub fn price(&self) -> Option<Price> {
        if let RoundState::Closed { result } = &self.state {
            Some(result.price)
        } else { None }
    }

    /// Is this round done (closed or failed)?
    pub fn is_done(&self) -> bool {
        matches!(self.state, RoundState::Closed { .. } | RoundState::Failed { .. })
    }
}

/// Manages rounds for all feeds.
pub struct RoundManager {
    rounds:      HashMap<FeedId, Vec<OracleRound>>,
    aggregator:  OracleAggregator,
    whitelist:   Vec<[u8; 20]>,
}

impl RoundManager {
    pub fn new(whitelist: Vec<[u8; 20]>, min_reporters: u32) -> Self {
        Self {
            rounds:     HashMap::new(),
            aggregator: OracleAggregator::new(min_reporters),
            whitelist,
        }
    }

    /// Open a new round for a feed.
    pub fn new_round(&mut self, feed_id: FeedId, now: u64) -> RoundId {
        let rounds = self.rounds.entry(feed_id.clone()).or_insert_with(Vec::new);
        let round_id = rounds.len() as u64 + 1;
        rounds.push(OracleRound::open(round_id, feed_id, now, 300));
        round_id
    }

    /// Submit a report to the current open round.
    pub fn submit(&mut self, feed_id: &FeedId, report: PriceReport, now: u64)
        -> Result<(), OracleError>
    {
        let rounds = self.rounds.get_mut(feed_id)
            .ok_or_else(|| OracleError::UnknownFeed(feed_id.clone()))?;
        let round = rounds.last_mut()
            .ok_or(OracleError::NoOpenRound)?;
        round.add_report(report, now, &self.whitelist)
    }

    /// Close the current round and return the aggregated price.
    pub fn close_round(&mut self, feed_id: &FeedId, now: u64)
        -> Result<Price, OracleError>
    {
        let rounds = self.rounds.get_mut(feed_id)
            .ok_or_else(|| OracleError::UnknownFeed(feed_id.clone()))?;
        let round = rounds.last_mut().ok_or(OracleError::NoOpenRound)?;
        let result = round.close(&self.aggregator, now, 3)?;
        Ok(result.price)
    }

    /// Latest round data for a feed (Chainlink AggregatorV3 compatible).
    pub fn latest_round_data(&self, feed_id: &FeedId)
        -> Option<(RoundId, Price, u64)>
    {
        self.rounds.get(feed_id)
            .and_then(|rounds| rounds.iter().rev()
                .find_map(|r| {
                    if let RoundState::Closed { result } = &r.state {
                        Some((r.round_id, result.price, result.timestamp))
                    } else { None }
                })
            )
    }
}