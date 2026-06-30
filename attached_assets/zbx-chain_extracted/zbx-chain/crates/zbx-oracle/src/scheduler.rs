//! Oracle scheduler — periodically opens rounds and fetches prices.

use crate::{
    feed::FeedId,
    round::RoundManager,
    reporter::OracleReporter,
    error::OracleError,
};
use std::time::{SystemTime, UNIX_EPOCH};

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

/// Drives one oracle update cycle: open round → fetch live price → submit
/// self-report → (other reporters submit independently) → ready to close.
///
/// # Parameters
/// - `feed_id`  — which price feed to update.
/// - `manager`  — the round manager tracking open/closed rounds.
/// - `reporter` — optional local reporter; when provided, a signed report is
///   submitted immediately after the live price is fetched.  When `None` the
///   round is opened but no self-report is submitted (external reporters still
///   submit through their own cycle calls).
///
/// # Errors
/// Returns `OracleError::AllSourcesFailed` if every configured CEX source is
/// unreachable.  Self-report submission failures are logged as warnings but
/// do **not** fail the cycle — other reporters may still complete the round.
pub async fn run_cycle(
    feed_id:  &FeedId,
    manager:  &mut RoundManager,
    reporter: Option<&OracleReporter>,
) -> Result<(), OracleError> {
    let now = now_secs();
    manager.new_round(feed_id.clone(), now);

    if let Some(r) = reporter {
        // Fetch live price and sign a report in one call.
        match r.report(feed_id, now).await {
            Ok(report) => {
                if let Err(e) = manager.submit(feed_id, report, now) {
                    tracing::warn!(
                        feed  = %feed_id,
                        error = %e,
                        "Self-report submission failed — other reporters may complete the round"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    feed  = %feed_id,
                    error = %e,
                    "Live price fetch failed for self-report"
                );
                return Err(e);
            }
        }
    }

    Ok(())
}
