//! Chainlink-style price oracle aggregator for ZBX Chain.
//! Off-chain reporters, on-chain aggregation, deviation check, round management.

use std::collections::{HashMap, BTreeMap};
use std::sync::RwLock;
use crate::types::{Address, U256};

/// Number of oracles required for a valid round
pub const MIN_ORACLE_COUNT: usize = 3;
/// Max oracles per feed
pub const MAX_ORACLES: usize = 31;
/// Max deviation before discarding answer (1000 bps = 10%)
pub const MAX_DEVIATION_BPS: u64 = 1000;
/// Round timeout: new round must be completed within this many seconds
pub const ROUND_TIMEOUT_SECS: u64 = 120;
/// Oracle payment per submission (in LINK/ZBX)
pub const ORACLE_PAYMENT: u64 = 10_000_000_000_000_000; // 0.01 ZBX

/// Chainlink round data
#[derive(Debug, Clone)]
pub struct RoundData {
    pub round_id: u64,
    pub answer: i128,         // price with oracle decimals
    pub started_at: u64,
    pub updated_at: u64,
    pub answered_in_round: u64,
}

/// Oracle submission in a round
#[derive(Debug, Clone)]
pub struct OracleSubmission {
    pub oracle: Address,
    pub answer: i128,
    pub timestamp: u64,
}

/// Feed state (one per price pair)
#[derive(Debug)]
pub struct FeedState {
    pub description: String,
    pub decimals: u8,
    pub version: u32,
    pub oracles: Vec<Address>,
    pub min_submissions: usize,
    pub max_submissions: usize,
    pub current_round: u64,
    pub latest_round_data: Option<RoundData>,
    pub pending_submissions: HashMap<u64, Vec<OracleSubmission>>,
    pub oracle_index: HashMap<Address, usize>,
    pub payments_owed: HashMap<Address, u64>,
}

impl FeedState {
    pub fn new(description: String, decimals: u8) -> Self {
        Self {
            description, decimals, version: 4,
            oracles: Vec::new(),
            min_submissions: MIN_ORACLE_COUNT,
            max_submissions: MAX_ORACLES,
            current_round: 0,
            latest_round_data: None,
            pending_submissions: HashMap::new(),
            oracle_index: HashMap::new(),
            payments_owed: HashMap::new(),
        }
    }

    /// Add oracle to the feed
    pub fn add_oracle(&mut self, oracle: Address) -> Result<(), OracleError> {
        if self.oracles.len() >= MAX_ORACLES { return Err(OracleError::TooManyOracles); }
        if self.oracle_index.contains_key(&oracle) { return Err(OracleError::AlreadyAdded(oracle)); }
        let idx = self.oracles.len();
        self.oracles.push(oracle);
        self.oracle_index.insert(oracle, idx);
        Ok(())
    }

    /// Oracle submits answer for a round
    pub fn submit(&mut self, oracle: Address, round_id: u64, answer: i128, timestamp: u64) -> Result<Option<RoundData>, OracleError> {
        if !self.oracle_index.contains_key(&oracle) {
            return Err(OracleError::NotOracle(oracle));
        }
        // Check already submitted
        let subs = self.pending_submissions.entry(round_id).or_default();
        if subs.iter().any(|s| s.oracle == oracle) {
            return Err(OracleError::AlreadySubmitted { oracle, round: round_id });
        }

        subs.push(OracleSubmission { oracle, answer, timestamp });

        // Check if we have enough submissions
        if subs.len() >= self.min_submissions {
            let result = self.aggregate_round(round_id, timestamp)?;
            return Ok(Some(result));
        }
        Ok(None)
    }

    /// Aggregate round submissions into final answer
    fn aggregate_round(&mut self, round_id: u64, timestamp: u64) -> Result<RoundData, OracleError> {
        let subs = self.pending_submissions.remove(&round_id)
            .ok_or(OracleError::RoundNotFound(round_id))?;

        let mut answers: Vec<i128> = subs.iter().map(|s| s.answer).collect();
        answers.sort_unstable();

        // Deviation filter: discard outliers beyond MAX_DEVIATION_BPS from median
        let median = answers[answers.len() / 2];
        let valid: Vec<i128> = answers.iter().copied()
            .filter(|&a| {
                let dev = if a > median { a - median } else { median - a };
                let dev_bps = if median != 0 { dev.unsigned_abs() * 10000 / median.unsigned_abs() } else { 0 };
                dev_bps <= MAX_DEVIATION_BPS as u128
            })
            .collect();

        if valid.len() < self.min_submissions {
            return Err(OracleError::InsufficientValidAnswers { valid: valid.len(), min: self.min_submissions });
        }

        // Final answer is median of valid answers
        let final_answer = valid[valid.len() / 2];
        let round_data = RoundData {
            round_id,
            answer: final_answer,
            started_at: subs.iter().map(|s| s.timestamp).min().unwrap_or(timestamp),
            updated_at: timestamp,
            answered_in_round: round_id,
        };

        // Pay oracles
        for sub in &subs {
            *self.payments_owed.entry(sub.oracle).or_default() += ORACLE_PAYMENT;
        }

        self.latest_round_data = Some(round_data.clone());
        self.current_round = round_id;

        tracing::info!(
            feed = %self.description,
            round = round_id,
            answer = final_answer,
            oracles = subs.len(),
            "Round aggregated"
        );

        Ok(round_data)
    }

    /// Get latest round data (Chainlink-compatible interface)
    pub fn latest_round_data(&self) -> Result<RoundData, OracleError> {
        self.latest_round_data.clone().ok_or(OracleError::NoData)
    }

    /// Get round data by ID
    pub fn get_round_data(&self, round_id: u64) -> Result<RoundData, OracleError> {
        // Only latest is cached; full history would require DB
        if let Some(ref rd) = self.latest_round_data {
            if rd.round_id == round_id { return Ok(rd.clone()); }
        }
        Err(OracleError::RoundNotFound(round_id))
    }
}

/// Chainlink oracle registry (all feeds)
pub struct ChainlinkOracle {
    pub feeds: RwLock<HashMap<String, FeedState>>,
    pub feed_by_address: RwLock<HashMap<Address, String>>,
}

impl ChainlinkOracle {
    pub fn new() -> Self {
        Self { feeds: RwLock::new(HashMap::new()), feed_by_address: RwLock::new(HashMap::new()) }
    }

    /// Register a new price feed
    pub fn add_feed(&self, key: String, description: String, decimals: u8, address: Address) {
        let mut feeds = self.feeds.write().unwrap();
        feeds.insert(key.clone(), FeedState::new(description, decimals));
        self.feed_by_address.write().unwrap().insert(address, key);
    }

    /// Oracle submits to a feed
    pub fn submit(&self, feed_key: &str, oracle: Address, answer: i128, timestamp: u64) -> Result<Option<RoundData>, OracleError> {
        let mut feeds = self.feeds.write().unwrap();
        let feed = feeds.get_mut(feed_key).ok_or(OracleError::FeedNotFound(feed_key.into()))?;
        let round_id = feed.current_round + 1;
        feed.submit(oracle, round_id, answer, timestamp)
    }

    /// Get latest price for a feed
    pub fn get_price(&self, feed_key: &str) -> Result<RoundData, OracleError> {
        self.feeds.read().unwrap()
            .get(feed_key)
            .ok_or_else(|| OracleError::FeedNotFound(feed_key.into()))?
            .latest_round_data()
    }
}

/// Oracle errors
#[derive(Debug, thiserror::Error)]
pub enum OracleError {
    #[error("Feed not found: {0}")]
    FeedNotFound(String),
    #[error("Not oracle: {0:?}")]
    NotOracle(Address),
    #[error("Already added: {0:?}")]
    AlreadyAdded(Address),
    #[error("Too many oracles")]
    TooManyOracles,
    #[error("Already submitted: oracle {oracle:?}, round {round}")]
    AlreadySubmitted { oracle: Address, round: u64 },
    #[error("Round not found: {0}")]
    RoundNotFound(u64),
    #[error("Insufficient valid answers: got {valid}, min {min}")]
    InsufficientValidAnswers { valid: usize, min: usize },
    #[error("No data available")]
    NoData,
}