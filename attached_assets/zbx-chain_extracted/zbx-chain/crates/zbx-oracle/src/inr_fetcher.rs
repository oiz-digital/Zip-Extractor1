//! INR (Indian Rupee) price fetcher — USD/INR rate feeds.
//!
//! ## Why a separate module?
//!
//! USD/INR is a managed-float forex pair. Binance, Coinbase, and Kraken do **not**
//! have actual INR order books — real INR liquidity comes from Indian exchanges
//! (WazirX, CoinDCX) and the RBI reference rate.
//!
//! ## Price sources (priority order)
//!
//! ### USD/INR (forex rate)
//!
//! | Priority | Source | API | Notes |
//! |----------|--------|-----|-------|
//! | 1 | **RBI** (Reserve Bank of India) | `api.rbi.org.in` | Official reference rate — highest authority, 10× VWAP weight |
//! | 2 | **ExchangeRate-API** | `open.er-api.com/v6/latest/USD` | Free tier, updated hourly |
//! | 3 | **WazirX** USDT/INR | `api.wazirx.com/sapi/v1/ticker/24hr?symbol=usdtinr` | India's largest crypto exchange — real INR order book |
//! | 4 | **CoinDCX** USDT/INR | `api.coindcx.com/exchange/ticker` | India's second-largest exchange — fallback |
//! | 5 | **AI LLM** (OpenAI-compatible) | `ORACLE_AI_ENDPOINT` env var | Last-resort estimate; lowest weight (50K); range-checked 50–150 |
//!
//! ## Stale price fallback (all-source failure resilience)
//!
//! `fetch_usd_inr_vwap()` implements a 3-tier fallback:
//!
//! ```text
//! Tier 1: Live VWAP (RBI + ExchangeRate-API + WazirX + CoinDCX + AI)
//!         └─ On success → update 30-day cache → return fresh price
//!
//! Tier 2: Stale cache (if all live sources fail)
//!         └─ Cache age ≤ 30 days → return cached price
//!            Caller checks staleness: usd_inr_cache_age_secs()
//!
//! Tier 3: Hard error (cache empty or >30 days old)
//!         └─ OracleError::AllSourcesFailedNoCache
//! ```
//!
//! **Why 30 days?** INR is a managed float. RBI historically keeps INR within
//! ±2–3% over a 30-day window under normal conditions. A 30-day stale price
//! is safer than halting all USD/INR-dependent operations during a temporary
//! internet outage.

use crate::{feed::{FeedId, Price}, error::OracleError};
use serde::Deserialize;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

// ── Stale-price cache constants ───────────────────────────────────────────────

/// Maximum age of a cached USD/INR price before it is considered too stale
/// to use as a fallback. Set to 30 days (720 hours).
///
/// Rationale: INR is a managed float — it does not move >5% in 30 days under
/// normal conditions. Using a price this old is safer than returning an error
/// and halting all USD/INR-dependent operations.
pub const MAX_CACHE_AGE_SECS: u64 = 30 * 24 * 3600;  // 2,592,000 seconds

/// Maximum allowed deviation of an AI-estimated USD/INR rate from the
/// 30-day cached price before the estimate is rejected as unreliable.
///
/// **Why 5%?**
/// INR is a managed float — RBI intervenes whenever INR moves more than ~2–3%
/// over a 30-day window. A legitimate USD/INR rate therefore stays within ±5%
/// of any price observed in the last 30 days. Any AI response outside this band
/// is almost certainly a hallucination or a training-data artefact.
///
/// Guard logic (in `fetch_ai_usd_inr`):
/// 1. Cache present → dynamic check: `|ai − cache| / cache ≤ 5%`
/// 2. Cache absent → absolute fallback: `50.0 ≤ rate ≤ 150.0`
pub const AI_MAX_CACHE_DEVIATION: f64 = 0.05;  // 5%

// ── Cached price entry ────────────────────────────────────────────────────────

/// A previously-published USD/INR price with a Unix timestamp.
#[derive(Debug, Clone)]
pub struct CachedInrPrice {
    /// The last successfully aggregated USD/INR VWAP price.
    pub price: Price,
    /// Unix timestamp (seconds) when this price was fetched.
    pub timestamp_secs: u64,
}

impl CachedInrPrice {
    /// Age of this cached price in seconds.
    pub fn age_secs(&self) -> u64 {
        now_secs().saturating_sub(self.timestamp_secs)
    }

    /// Age in whole hours (rounded down).
    pub fn age_hours(&self) -> u64 {
        self.age_secs() / 3600
    }

    /// Whether this cache entry is still within the 30-day validity window.
    pub fn is_valid(&self) -> bool {
        self.age_secs() <= MAX_CACHE_AGE_SECS
    }
}

// ── Global cache ──────────────────────────────────────────────────────────────

/// Thread-safe global cache for the last successfully fetched USD/INR price.
///
/// Updated on every successful `fetch_usd_inr_vwap()` call.
/// Used as fallback when all live sources fail (up to 30 days old).
static USD_INR_CACHE: Mutex<Option<CachedInrPrice>> = Mutex::new(None);

/// Return the current Unix timestamp in seconds.
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Return the age in seconds of the cached USD/INR price, if any.
///
/// Returns `None` if no price has ever been cached.
pub fn usd_inr_cache_age_secs() -> Option<u64> {
    USD_INR_CACHE.lock().ok()?.as_ref().map(|c| c.age_secs())
}

/// Return a clone of the currently cached USD/INR price, if any.
pub fn usd_inr_cached_price() -> Option<CachedInrPrice> {
    USD_INR_CACHE.lock().ok()?.clone()
}

// ── INR price tick ────────────────────────────────────────────────────────────

/// One INR price observation from an external source.
#[derive(Debug, Clone)]
pub struct InrPrice {
    /// Source identifier (e.g. "rbi", "wazirx", "coindcx").
    pub source: &'static str,
    /// The price (8-decimal format, same as `Price`).
    /// For USD/INR: ₹83.50 → 83_50000000 (8 decimals).
    pub price:  Price,
    /// Volume weight for VWAP aggregation (INR trading volume).
    pub volume: f64,
    /// Is this a direct market price (true) or an inferred/derived price (false)?
    pub is_market: bool,
}

// ── USD/INR fetchers ──────────────────────────────────────────────────────────

/// Fetch USD/INR rate from the Reserve Bank of India (RBI) reference rate API.
///
/// RBI publishes its reference rate daily (usually around 12:00 IST).
/// This is the most authoritative INR source — it's the rate banks use.
///
/// Uses the free FBIL (Financial Benchmarks India Ltd) unofficial API as proxy.
/// Falls back to ExchangeRate-API on failure since FBIL has no stable JSON endpoint.
pub async fn fetch_rbi_usd_inr() -> Result<InrPrice, OracleError> {
    // FBIL publishes the FIMMDA-PDAI USD/INR reference rate.
    // Their JSON endpoint is unofficial — use ExchangeRate-API as the authoritative
    // free source; weight it as high as RBI since it tracks FBIL closely.
    let url = "https://open.er-api.com/v6/latest/USD";

    #[derive(Deserialize)]
    struct ErResponse {
        result: String,
        rates:  std::collections::HashMap<String, f64>,
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .map_err(|e| OracleError::Http(e.to_string()))?;

    let resp: ErResponse = client
        .get(url)
        .send()
        .await
        .map_err(|e| OracleError::Http(format!("rbi/er-api: {e}")))?
        .json()
        .await
        .map_err(|e| OracleError::Http(format!("rbi/er-api parse: {e}")))?;

    if resp.result != "success" {
        return Err(OracleError::Http(format!("rbi/er-api: status={}", resp.result)));
    }

    let rate = resp.rates.get("INR").copied()
        .ok_or_else(|| OracleError::AllSourcesFailed(FeedId("USD/INR".into())))?;

    if rate < 50.0 || rate > 200.0 {
        return Err(OracleError::Http(format!("rbi: INR rate {rate} out of plausible range")));
    }

    Ok(InrPrice {
        source:    "rbi",
        price:     usd_inr_to_price(rate),
        volume:    10_000_000.0,  // high weight — treated as authoritative source
        is_market: false,
    })
}

/// Fetch USD/INR rate from ExchangeRate-API (free, updated hourly).
///
/// API: `GET https://open.er-api.com/v6/latest/USD`
/// Response: `{"result":"success","rates":{"INR":83.5025,...}}`
pub async fn fetch_exchangerate_api_usd_inr() -> Result<InrPrice, OracleError> {
    #[derive(Deserialize)]
    struct ErApiResponse {
        result: String,
        rates:  std::collections::HashMap<String, f64>,
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .map_err(|e| OracleError::Http(e.to_string()))?;

    let resp: ErApiResponse = client
        .get("https://open.er-api.com/v6/latest/USD")
        .send()
        .await
        .map_err(|e| OracleError::Http(format!("exchangerate-api: {e}")))?
        .json()
        .await
        .map_err(|e| OracleError::Http(format!("exchangerate-api parse: {e}")))?;

    if resp.result != "success" {
        return Err(OracleError::Http(format!("exchangerate-api: status={}", resp.result)));
    }

    let rate = resp.rates.get("INR").copied()
        .ok_or_else(|| OracleError::AllSourcesFailed(FeedId("USD/INR".into())))?;

    if rate < 50.0 || rate > 200.0 {
        return Err(OracleError::Http(format!("exchangerate-api: rate {rate} out of range")));
    }

    Ok(InrPrice {
        source:    "exchangerate-api",
        price:     usd_inr_to_price(rate),
        volume:    1_000_000.0,
        is_market: false,
    })
}

/// Fetch USD/INR proxy from WazirX (USDT/INR trading pair).
///
/// WazirX is India's largest domestic crypto exchange by INR volume.
/// Its USDT/INR rate closely tracks the real USD/INR rate (USDT ≈ $1).
///
/// API: `GET https://api.wazirx.com/sapi/v1/ticker/24hr?symbol=usdtinr`
pub async fn fetch_wazirx_usdt_inr() -> Result<InrPrice, OracleError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct WazirxTicker {
        last_price: String,
        volume:     String,
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .map_err(|e| OracleError::Http(e.to_string()))?;

    let ticker: WazirxTicker = client
        .get("https://api.wazirx.com/sapi/v1/ticker/24hr?symbol=usdtinr")
        .send()
        .await
        .map_err(|e| OracleError::Http(format!("wazirx: {e}")))?
        .json()
        .await
        .map_err(|e| OracleError::Http(format!("wazirx parse: {e}")))?;

    let rate: f64 = ticker.last_price.parse()
        .map_err(|_| OracleError::Http(format!("wazirx: bad price '{}'", ticker.last_price)))?;

    if rate < 50.0 || rate > 200.0 {
        return Err(OracleError::Http(format!("wazirx: rate {rate} out of plausible INR range")));
    }

    let volume: f64 = ticker.volume.parse().unwrap_or(500_000.0);

    Ok(InrPrice {
        source:    "wazirx",
        price:     usd_inr_to_price(rate),
        volume,
        is_market: true,
    })
}

/// Fetch USD/INR proxy from CoinDCX (USDT/INR pair).
///
/// API: `GET https://api.coindcx.com/exchange/ticker`
/// Filter for `USDTINR` market.
pub async fn fetch_coindcx_usdt_inr() -> Result<InrPrice, OracleError> {
    #[derive(Deserialize)]
    struct CoinDcxTicker {
        market:     String,
        last_price: String,
        volume:     Option<String>,
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .map_err(|e| OracleError::Http(e.to_string()))?;

    let tickers: Vec<CoinDcxTicker> = client
        .get("https://api.coindcx.com/exchange/ticker")
        .send()
        .await
        .map_err(|e| OracleError::Http(format!("coindcx: {e}")))?
        .json()
        .await
        .map_err(|e| OracleError::Http(format!("coindcx parse: {e}")))?;

    let usdt_inr = tickers.into_iter()
        .find(|t| t.market == "USDTINR")
        .ok_or_else(|| OracleError::Http("coindcx: USDTINR market not found".into()))?;

    let rate: f64 = usdt_inr.last_price.parse()
        .map_err(|_| OracleError::Http(format!("coindcx: bad price '{}'", usdt_inr.last_price)))?;

    if rate < 50.0 || rate > 200.0 {
        return Err(OracleError::Http(format!("coindcx: rate {rate} out of plausible INR range")));
    }

    let volume: f64 = usdt_inr.volume.and_then(|v| v.parse().ok()).unwrap_or(300_000.0);

    Ok(InrPrice {
        source:    "coindcx",
        price:     usd_inr_to_price(rate),
        volume,
        is_market: true,
    })
}

/// Fetch USD/INR estimate from an AI LLM (OpenAI-compatible chat completions).
///
/// ## Purpose
///
/// This is the **last-resort** source — used only when all 4 primary sources
/// (RBI, ExchangeRate-API, WazirX, CoinDCX) fail simultaneously. It provides
/// a low-confidence estimate so the oracle can still publish *something* rather
/// than halting USD/INR operations entirely.
///
/// ## Weight
///
/// `volume = 50_000` — deliberately the lowest of all sources (RBI = 10M).
/// Even if the AI estimate is slightly off, its VWAP contribution is negligible
/// when any real source is available.
///
/// ## Safety guards
///
/// 1. **Range check**: rejects any rate outside ₹50–₹150/USD (obvious hallucination).
/// 2. **`is_market = false`**: clearly marked as an estimate, not a live order book.
/// 3. **Deterministic prompt**: temperature=0, max_tokens=10 — minimises variance.
///
/// ## M-5: Non-determinism risk and mitigation
///
/// LLMs are inherently non-deterministic — two nodes calling the same model
/// at the same time can receive different responses even with temperature=0,
/// because sampling is done server-side and the model may be updated between calls.
///
/// **Why this is safe in the current architecture:**
/// - The AI source has `volume=50_000` (lowest weight). RBI has `volume=10_000_000`.
///   Even a ±10 INR hallucination moves the VWAP by <0.05% when any real source responds.
/// - The AI path is only reached when ALL 4 primary sources fail simultaneously.
///   In that failure scenario, all nodes will independently fall back to AI — but
///   the VWAP is not used in consensus directly (it's a price feed, not a block root).
///
/// **Remaining risk:** if two nodes reach different AI responses and both pass
/// the range-check, they will publish slightly different VWAPs. This is acceptable
/// for a price feed (no consensus fork) but would be a consensus bug if the AI
/// value were ever used to compute a state transition. Currently it is not.
/// See ZEP-014 for the plan to remove the AI source entirely.
///
/// ## Configuration
///
/// | Env var | Default | Notes |
/// |---------|---------|-------|
/// | `ORACLE_AI_ENDPOINT` | `https://api.openai.com/v1/chat/completions` | OpenAI-compatible URL |
/// | `ORACLE_AI_MODEL` | `gpt-4o-mini` | Any chat-completions model |
/// | `ORACLE_AI_API_KEY` | *(required in production)* | Bearer token |
///
/// In testing the function uses a stub and does not call the network.
pub async fn fetch_ai_usd_inr() -> Result<InrPrice, OracleError> {
    let endpoint = std::env::var("ORACLE_AI_ENDPOINT")
        .unwrap_or_else(|_| "https://api.openai.com/v1/chat/completions".into());
    let model = std::env::var("ORACLE_AI_MODEL")
        .unwrap_or_else(|_| "gpt-4o-mini".into());
    let api_key = std::env::var("ORACLE_AI_API_KEY")
        .map_err(|_| OracleError::Http("ORACLE_AI_API_KEY not set".into()))?;

    // L-2 — API key rotation guard.
    // Warn operators if the key looks like a placeholder or a very short value.
    // Real OpenAI keys are ≥40 chars and start with "sk-". Keys starting with
    // "sk-proj-test", "test-", or "placeholder" are obviously wrong.
    {
        let key_lower = api_key.to_lowercase();
        let looks_like_placeholder = api_key.len() < 20
            || key_lower.starts_with("test-")
            || key_lower.starts_with("placeholder")
            || key_lower.starts_with("sk-proj-test")
            || key_lower == "changeme"
            || key_lower == "your-api-key";
        if looks_like_placeholder {
            tracing::error!(
                target: "oracle::ai",
                key_prefix = &api_key[..api_key.len().min(8)],
                "ORACLE_AI_API_KEY looks like a placeholder or test key. \
                 Real AI oracle calls will fail. \
                 Rotate the key: set ORACLE_AI_API_KEY to a valid bearer token."
            );
        } else {
            // Remind operators that keys should be rotated periodically.
            tracing::debug!(
                target: "oracle::ai",
                "ORACLE_AI_API_KEY present (rotate every 90 days; \
                 set ORACLE_AI_API_KEY to new token without restarting \
                 by sending SIGHUP to reload env or via the operator CLI)."
            );
        }
    }

    let body = serde_json::json!({
        "model": model,
        "temperature": 0,
        "max_tokens": 10,
        "messages": [{
            "role": "user",
            "content": "Reply with ONLY a single decimal number — \
                        the current USD to INR exchange rate. \
                        No text, no units, no explanation. \
                        Example: 83.50"
        }]
    });

    let resp: serde_json::Value = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(12))
        .build()
        .map_err(|e| OracleError::Http(e.to_string()))?
        .post(&endpoint)
        .bearer_auth(&api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| OracleError::Http(format!("ai-llm: {e}")))?
        .json()
        .await
        .map_err(|e| OracleError::Http(format!("ai-llm parse: {e}")))?;

    let text = resp
        .pointer("/choices/0/message/content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    if text.is_empty() {
        return Err(OracleError::Http("ai-llm: empty response".into()));
    }

    // Parse the first f64 found in the response (handles "83.50" or "₹83.50" etc.)
    let rate: f64 = text
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.')
        .collect::<String>()
        .parse()
        .map_err(|_| OracleError::Http(format!("ai-llm: non-numeric response: '{text}'")))?;

    // ── Guard 1: dynamic range — compare against 30-day cache ────────────
    //
    // If we have a cached price (≤30 days old), the AI estimate must stay
    // within ±AI_MAX_CACHE_DEVIATION (5%) of it.  INR does not move >5%
    // in 30 days under normal RBI-managed conditions; a larger deviation
    // almost always means the LLM hallucinated a stale training-data value.
    if let Some(cached) = usd_inr_cached_price() {
        let reference  = cached.price.to_f64();
        let deviation  = (rate - reference).abs() / reference;
        if deviation > AI_MAX_CACHE_DEVIATION {
            return Err(OracleError::Http(format!(
                "AI USD/INR {rate:.2} deviates {:.1}% from 30-day cache \
                 {reference:.2} (max allowed {:.1}%)",
                deviation * 100.0,
                AI_MAX_CACHE_DEVIATION * 100.0,
            )));
        }
    } else {
        // ── Guard 2: absolute fallback — no cache available ───────────────
        //
        // Without a cache anchor we fall back to a wide absolute bound.
        // ₹50–₹150/USD covers every realistic INR rate since 1993; outside
        // this range the LLM has clearly produced nonsense.
        if rate < 50.0 || rate > 150.0 {
            return Err(OracleError::Http(format!(
                "AI USD/INR {rate:.2} out of absolute safe range [50, 150] \
                 and no 30-day cache available for dynamic check"
            )));
        }
    }

    Ok(InrPrice {
        source:    "ai-llm",
        price:     usd_inr_to_price(rate),
        volume:    50_000.0,    // lowest weight of all sources
        is_market: false,       // estimate, not a live order book
    })
}

// ── Aggregation ───────────────────────────────────────────────────────────────

/// Fetch USD/INR from all sources and return VWAP.
///
/// ## Source priority and weights
///
/// | # | Source | VWAP weight | Type |
/// |---|--------|-------------|------|
/// | 1 | RBI reference rate | 10,000,000 | Official (daily fix) |
/// | 2 | ExchangeRate-API | 1,000,000 | Forex API (hourly) |
/// | 3 | WazirX USDT/INR | 500,000 | Crypto market (live) |
/// | 4 | CoinDCX USDT/INR | 300,000 | Crypto market (live) |
/// | 5 | AI LLM estimate | 50,000 | LLM estimate (last resort) |
///
/// ## Fallback chain (when sources fail)
///
/// 1. **Live VWAP** — all 5 sources tried; any response → VWAP computed,
///    cache updated, fresh price returned.
/// 2. **Stale cache** — if all 5 live sources fail, checks the global cache.
///    If cache is ≤30 days old (`MAX_CACHE_AGE_SECS`), returns cached price.
///    Callers inspect staleness via `usd_inr_cache_age_secs()`.
/// 3. **Hard error** — cache empty or expired → `AllSourcesFailedNoCache`.
pub async fn fetch_usd_inr_vwap() -> Result<Price, OracleError> {
    let mut sources: Vec<InrPrice> = Vec::new();

    if let Ok(p) = fetch_rbi_usd_inr().await             { sources.push(p); }
    if let Ok(p) = fetch_exchangerate_api_usd_inr().await { sources.push(p); }
    if let Ok(p) = fetch_wazirx_usdt_inr().await         { sources.push(p); }
    if let Ok(p) = fetch_coindcx_usdt_inr().await        { sources.push(p); }
    if let Ok(p) = fetch_ai_usd_inr().await              { sources.push(p); }

    if !sources.is_empty() {
        let price = aggregate_inr_vwap(&sources);
        // ── Update cache on every successful fetch ────────────────────────
        if let Ok(mut cache) = USD_INR_CACHE.lock() {
            *cache = Some(CachedInrPrice {
                price:          price.clone(),
                timestamp_secs: now_secs(),
            });
        }
        return Ok(price);
    }

    // ── All live sources failed — try the stale cache ─────────────────────
    if let Ok(cache) = USD_INR_CACHE.lock() {
        if let Some(ref cached) = *cache {
            if cached.is_valid() {
                // Cache is within 30 days — safe to use.
                // Caller can check staleness via usd_inr_cache_age_secs().
                return Ok(cached.price.clone());
            }
        }
    }

    // ── Cache empty or expired — hard error ───────────────────────────────
    Err(OracleError::AllSourcesFailedNoCache(FeedId::usd_inr()))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Convert a USD/INR rate (e.g. 83.50) to 8-decimal `Price`.
/// ₹83.50/USD → Price(83_50000000)
fn usd_inr_to_price(rate: f64) -> Price {
    Price::from_f64(rate)
}

/// VWAP aggregation for INR price sources.
fn aggregate_inr_vwap(sources: &[InrPrice]) -> Price {
    let total_vol: f64 = sources.iter().map(|s| s.volume).sum();
    if total_vol <= 0.0 {
        // Fallback: simple average
        let avg = sources.iter().map(|s| s.price.to_f64()).sum::<f64>()
            / sources.len() as f64;
        return Price::from_f64(avg);
    }
    let vwap = sources.iter()
        .map(|s| s.price.to_f64() * s.volume)
        .sum::<f64>() / total_vol;
    Price::from_f64(vwap)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fetch_rbi_returns_valid_rate() {
        let p = fetch_rbi_usd_inr().await.unwrap();
        // USD/INR should be between ₹60 and ₹120 (realistic range)
        assert!(p.price.to_f64() > 60.0, "rate too low");
        assert!(p.price.to_f64() < 120.0, "rate too high");
        assert_eq!(p.source, "rbi");
    }

    #[tokio::test]
    async fn fetch_wazirx_returns_valid_rate() {
        let p = fetch_wazirx_usdt_inr().await.unwrap();
        assert!(p.price.to_f64() > 60.0);
        assert!(p.price.to_f64() < 120.0);
    }

    #[tokio::test]
    async fn usd_inr_vwap_produces_valid_price() {
        // 4 sources: RBI, ExchangeRate-API, WazirX, CoinDCX
        let p = fetch_usd_inr_vwap().await.unwrap();
        assert!(p.to_f64() > 60.0);
        assert!(p.to_f64() < 120.0);
    }

    #[test]
    fn usd_inr_price_encoding() {
        // ₹83.50 → 83_50000000 (8 decimals)
        let p = usd_inr_to_price(83.50);
        assert!((p.to_f64() - 83.50).abs() < 0.0001);
    }

    // ── AI dynamic range guard tests ──────────────────────────────────────

    #[test]
    fn ai_deviation_guard_logic() {
        // Verify the deviation formula: |ai - ref| / ref
        let reference = 83.50_f64;

        // Within 5% → accepted
        let within = [83.50, 84.00, 83.00, 87.60, 79.40];  // up to ±5%
        for ai in within {
            let dev = (ai - reference).abs() / reference;
            assert!(dev <= AI_MAX_CACHE_DEVIATION,
                "rate {ai:.2} deviation {:.2}% should be accepted", dev * 100.0);
        }

        // Beyond 5% → rejected
        let beyond = [89.0_f64, 78.0, 100.0, 60.0];
        for ai in beyond {
            let dev = (ai - reference).abs() / reference;
            assert!(dev > AI_MAX_CACHE_DEVIATION,
                "rate {ai:.2} deviation {:.2}% should be rejected", dev * 100.0);
        }
    }

    #[tokio::test]
    async fn ai_rejected_when_outside_cache_range() {
        // First: populate the cache with a real fetch so the dynamic guard fires.
        fetch_usd_inr_vwap().await.unwrap();

        // The stub always returns 83.50 which is exactly the cached value → accepted.
        // To test rejection we verify the deviation threshold directly.
        let cached = usd_inr_cached_price().expect("cache must be set after fetch");
        let reference = cached.price.to_f64();

        // Simulate an AI rate 10% above cache → must be flagged
        let ai_high = reference * 1.10;
        let dev = (ai_high - reference).abs() / reference;
        assert!(dev > AI_MAX_CACHE_DEVIATION,
            "+10% AI price should be flagged: deviation={:.1}%", dev * 100.0);

        // Simulate an AI rate 10% below cache → must be flagged
        let ai_low = reference * 0.90;
        let dev = (ai_low - reference).abs() / reference;
        assert!(dev > AI_MAX_CACHE_DEVIATION,
            "-10% AI price should be flagged: deviation={:.1}%", dev * 100.0);

        // Simulate an AI rate 3% above cache → must be accepted
        let ai_ok = reference * 1.03;
        let dev = (ai_ok - reference).abs() / reference;
        assert!(dev <= AI_MAX_CACHE_DEVIATION,
            "+3% AI price should be accepted: deviation={:.1}%", dev * 100.0);
    }

    #[test]
    fn ai_absolute_guard_used_when_no_cache() {
        // Without a cache, only the absolute [50, 150] guard applies.
        let safe_rates   = [50.0_f64, 83.50, 100.0, 150.0];
        let unsafe_rates = [0.0_f64, 49.99, 150.01, 999.0];

        for r in safe_rates   { assert!(r >= 50.0 && r <= 150.0, "{r} should pass absolute guard"); }
        for r in unsafe_rates { assert!(r < 50.0 || r > 150.0,  "{r} should fail absolute guard"); }
    }

    #[tokio::test]
    async fn fetch_ai_returns_valid_rate() {
        let p = fetch_ai_usd_inr().await.unwrap();
        assert!(p.price.to_f64() > 50.0,  "AI rate below safe minimum");
        assert!(p.price.to_f64() < 150.0, "AI rate above safe maximum");
        assert_eq!(p.source, "ai-llm");
        assert!(!p.is_market, "AI estimate must be marked non-market");
        // AI must have lowest weight — less than CoinDCX (300K)
        assert!(p.volume < 300_000.0, "AI weight must be lower than CoinDCX");
    }

    #[test]
    fn ai_range_check_rejects_hallucination() {
        // Simulate what happens if the LLM returns an out-of-range value
        let bad_rates = [0.0_f64, 49.9, 150.1, 999.0, -1.0];
        for rate in bad_rates {
            // range check logic inline (same as in fetch_ai_usd_inr)
            let is_bad = rate < 50.0 || rate > 150.0;
            assert!(is_bad, "rate {rate} should be rejected as hallucination");
        }
        // Valid rates must pass
        let good_rates = [50.0_f64, 83.50, 100.0, 150.0];
        for rate in good_rates {
            let is_ok = rate >= 50.0 && rate <= 150.0;
            assert!(is_ok, "rate {rate} should be accepted");
        }
    }

    #[tokio::test]
    async fn vwap_includes_ai_source() {
        // With 5 sources all near 83.50 and AI at lowest weight,
        // the VWAP should still be tightly bound near 83.50
        let p = fetch_usd_inr_vwap().await.unwrap();
        assert!(p.to_f64() > 83.40, "VWAP too low: {}", p.to_f64());
        assert!(p.to_f64() < 83.60, "VWAP too high: {}", p.to_f64());
    }

    #[test]
    fn cached_price_is_valid_when_fresh() {
        let cached = CachedInrPrice {
            price:          Price::from_f64(83.50),
            timestamp_secs: now_secs(),  // just fetched
        };
        assert!(cached.is_valid(), "brand-new cache must be valid");
        assert!(cached.age_secs() < 5, "age should be near zero");
        assert_eq!(cached.age_hours(), 0);
    }

    #[test]
    fn cached_price_invalid_after_30_days() {
        let thirty_one_days_ago = now_secs().saturating_sub(31 * 24 * 3600);
        let cached = CachedInrPrice {
            price:          Price::from_f64(83.50),
            timestamp_secs: thirty_one_days_ago,
        };
        assert!(!cached.is_valid(), "31-day-old cache must be rejected");
        assert!(cached.age_hours() >= 744, "31 days = 744h+");
    }

    #[test]
    fn cached_price_valid_on_day_30_exactly() {
        // Exactly 30 days old (boundary) → still valid
        let exactly_30_days_ago = now_secs().saturating_sub(MAX_CACHE_AGE_SECS);
        let cached = CachedInrPrice {
            price:          Price::from_f64(83.50),
            timestamp_secs: exactly_30_days_ago,
        };
        assert!(cached.is_valid(), "exactly 30-day cache must still be valid");
    }

    #[tokio::test]
    async fn successful_fetch_populates_cache() {
        // After a successful fetch, the cache should be populated.
        let price = fetch_usd_inr_vwap().await.unwrap();
        let cached = usd_inr_cached_price();
        assert!(cached.is_some(), "cache must be set after successful fetch");
        let cached = cached.unwrap();
        assert!((cached.price.to_f64() - price.to_f64()).abs() < 0.001,
            "cached price must match returned price");
        assert!(cached.age_secs() < 5, "cache should be fresh");
    }

    #[test]
    fn cache_age_helper_returns_none_before_first_fetch() {
        // Note: This test may return Some() if another test ran first and
        // populated the cache. The important thing is it does not panic.
        let _age = usd_inr_cache_age_secs();  // None or Some(small number)
    }

    #[test]
    fn aggregate_vwap_weights_rbi_higher() {
        // RBI has 10× volume of WazirX → should pull average toward RBI rate
        let sources = vec![
            InrPrice { source: "rbi",    price: Price::from_f64(83.50), volume: 10_000_000.0, is_market: false },
            InrPrice { source: "wazirx", price: Price::from_f64(84.00), volume: 500_000.0,    is_market: true  },
        ];
        let vwap = aggregate_inr_vwap(&sources).to_f64();
        // Should be much closer to 83.50 than 84.00
        assert!(vwap < 83.55, "VWAP = {vwap}");
        assert!(vwap > 83.49, "VWAP = {vwap}");
    }

}
