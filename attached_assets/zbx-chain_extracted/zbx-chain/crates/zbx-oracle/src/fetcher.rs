//! External price fetcher — calls multiple CEX APIs and aggregates.
//!
//! Sources used (with fallback priority):
//!   1. Binance   (largest volume, most reliable)
//!   2. Coinbase  (second fallback)
//!   3. Kraken    (third fallback)
//!   4. Gate.io   (fourth fallback)
//!   5. Bybit     (fifth fallback)
//!   6. KuCoin    (sixth fallback)
//!   7. CoinGecko (aggregator cross-check)
//!
//! Aggregation: VWAP (volume-weighted average price) across sources.
//! If any source fails: falls back to simple median of available prices.

use crate::{feed::{FeedId, Price}, error::OracleError};
use serde::Deserialize;

/// HTTP client timeout for all price fetches.
const FETCH_TIMEOUT_SECS: u64 = 8;

/// Build a shared reqwest client with timeout and rustls-tls.
fn http_client() -> Result<reqwest::Client, OracleError> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS))
        .build()
        .map_err(|e| OracleError::Http(e.to_string()))
}

/// One price tick from an external source.
#[derive(Debug, Clone)]
pub struct ExternalPrice {
    pub source: &'static str,
    pub price:  Price,
    pub volume: f64,
}

/// Fetch current price for a symbol from Binance spot API.
///
/// Endpoint: `GET https://api.binance.com/api/v3/ticker/24hr?symbol={BASE}USDT`
pub async fn fetch_binance(symbol: &str) -> Result<ExternalPrice, OracleError> {
    let base = symbol.split('/').next().unwrap_or(symbol);
    let url = format!("https://api.binance.com/api/v3/ticker/24hr?symbol={}USDT", base);

    #[derive(Deserialize)]
    struct BinanceTicker {
        #[serde(rename = "lastPrice")]
        price: String,
        #[serde(rename = "volume")]
        volume: String,
    }

    let ticker: BinanceTicker = http_client()?
        .get(&url)
        .send()
        .await
        .map_err(|e| OracleError::Http(format!("binance: {e}")))?
        .json()
        .await
        .map_err(|e| OracleError::Http(format!("binance parse: {e}")))?;

    let price_f: f64 = ticker.price.parse()
        .map_err(|_| OracleError::Http(format!("binance: bad price '{}'", ticker.price)))?;
    if price_f <= 0.0 {
        return Err(OracleError::InvalidPrice(0));
    }
    let volume_f: f64 = ticker.volume.parse().unwrap_or(1_000_000.0);

    Ok(ExternalPrice { source: "binance", price: Price::from_f64(price_f), volume: volume_f })
}

/// Fetch current price from Coinbase Advanced Trade API.
///
/// Endpoint: `GET https://api.coinbase.com/v2/prices/{BASE}-USD/spot`
pub async fn fetch_coinbase(symbol: &str) -> Result<ExternalPrice, OracleError> {
    let base = symbol.split('/').next().unwrap_or(symbol);
    let url = format!("https://api.coinbase.com/v2/prices/{base}-USD/spot");

    #[derive(Deserialize)]
    struct CbData { amount: String }
    #[derive(Deserialize)]
    struct CbResponse { data: CbData }

    let resp: CbResponse = http_client()?
        .get(&url)
        .send()
        .await
        .map_err(|e| OracleError::Http(format!("coinbase: {e}")))?
        .json()
        .await
        .map_err(|e| OracleError::Http(format!("coinbase parse: {e}")))?;

    let price_f: f64 = resp.data.amount.parse()
        .map_err(|_| OracleError::Http(format!("coinbase: bad price '{}'", resp.data.amount)))?;
    if price_f <= 0.0 {
        return Err(OracleError::InvalidPrice(0));
    }

    Ok(ExternalPrice { source: "coinbase", price: Price::from_f64(price_f), volume: 500_000.0 })
}

/// Fetch current price from Kraken REST API.
///
/// Endpoint: `GET https://api.kraken.com/0/public/Ticker?pair={BASE}USD`
pub async fn fetch_kraken(symbol: &str) -> Result<ExternalPrice, OracleError> {
    let base = symbol.split('/').next().unwrap_or(symbol);
    let pair = format!("{base}USD");
    let url = format!("https://api.kraken.com/0/public/Ticker?pair={pair}");

    #[derive(Deserialize)]
    struct KrakenTicker {
        c: Vec<String>,  // last trade: [price, volume]
        v: Vec<String>,  // volume: [today, last-24h]
    }
    #[derive(Deserialize)]
    struct KrakenResponse {
        error: Vec<String>,
        result: Option<std::collections::HashMap<String, KrakenTicker>>,
    }

    let resp: KrakenResponse = http_client()?
        .get(&url)
        .send()
        .await
        .map_err(|e| OracleError::Http(format!("kraken: {e}")))?
        .json()
        .await
        .map_err(|e| OracleError::Http(format!("kraken parse: {e}")))?;

    if !resp.error.is_empty() {
        return Err(OracleError::Http(format!("kraken error: {:?}", resp.error)));
    }

    let result = resp.result.ok_or_else(|| OracleError::Http("kraken: empty result".into()))?;
    let ticker = result.values().next()
        .ok_or_else(|| OracleError::Http("kraken: no ticker in result".into()))?;

    let price_f: f64 = ticker.c.first()
        .ok_or_else(|| OracleError::Http("kraken: missing last trade".into()))?
        .parse()
        .map_err(|_| OracleError::Http("kraken: bad price".into()))?;
    if price_f <= 0.0 {
        return Err(OracleError::InvalidPrice(0));
    }
    let volume_f: f64 = ticker.v.get(1).and_then(|v| v.parse().ok()).unwrap_or(300_000.0);

    Ok(ExternalPrice { source: "kraken", price: Price::from_f64(price_f), volume: volume_f })
}

/// Fetch current price from Gate.io.
///
/// Endpoint: `GET https://api.gateio.ws/api/v4/spot/tickers?currency_pair={BASE}_USDT`
pub async fn fetch_gate(symbol: &str) -> Result<ExternalPrice, OracleError> {
    let base = symbol.split('/').next().unwrap_or(symbol);
    let pair = format!("{base}_USDT");
    let url = format!("https://api.gateio.ws/api/v4/spot/tickers?currency_pair={pair}");

    #[derive(Deserialize)]
    struct GateTicker {
        last: String,
        base_volume: String,
    }

    let tickers: Vec<GateTicker> = http_client()?
        .get(&url)
        .send()
        .await
        .map_err(|e| OracleError::Http(format!("gate: {e}")))?
        .json()
        .await
        .map_err(|e| OracleError::Http(format!("gate parse: {e}")))?;

    let ticker = tickers.into_iter().next()
        .ok_or_else(|| OracleError::Http("gate: no ticker found".into()))?;

    let price_f: f64 = ticker.last.parse()
        .map_err(|_| OracleError::Http(format!("gate: bad price '{}'", ticker.last)))?;
    if price_f <= 0.0 {
        return Err(OracleError::InvalidPrice(0));
    }
    let volume_f: f64 = ticker.base_volume.parse().unwrap_or(200_000.0);

    Ok(ExternalPrice { source: "gate", price: Price::from_f64(price_f), volume: volume_f })
}

/// Fetch current price from Bybit spot market.
///
/// Endpoint: `GET https://api.bybit.com/v5/market/tickers?category=spot&symbol={BASE}USDT`
pub async fn fetch_bybit(symbol: &str) -> Result<ExternalPrice, OracleError> {
    let base = symbol.split('/').next().unwrap_or(symbol);
    let sym = format!("{base}USDT");
    let url = format!("https://api.bybit.com/v5/market/tickers?category=spot&symbol={sym}");

    #[derive(Deserialize)]
    struct BybitItem {
        #[serde(rename = "lastPrice")]
        last_price: String,
        #[serde(rename = "volume24h")]
        volume_24h: String,
    }
    #[derive(Deserialize)]
    struct BybitList { list: Vec<BybitItem> }
    #[derive(Deserialize)]
    struct BybitResponse { result: BybitList }

    let resp: BybitResponse = http_client()?
        .get(&url)
        .send()
        .await
        .map_err(|e| OracleError::Http(format!("bybit: {e}")))?
        .json()
        .await
        .map_err(|e| OracleError::Http(format!("bybit parse: {e}")))?;

    let item = resp.result.list.into_iter().next()
        .ok_or_else(|| OracleError::Http("bybit: no ticker found".into()))?;

    let price_f: f64 = item.last_price.parse()
        .map_err(|_| OracleError::Http(format!("bybit: bad price '{}'", item.last_price)))?;
    if price_f <= 0.0 {
        return Err(OracleError::InvalidPrice(0));
    }
    let volume_f: f64 = item.volume_24h.parse().unwrap_or(250_000.0);

    Ok(ExternalPrice { source: "bybit", price: Price::from_f64(price_f), volume: volume_f })
}

/// Fetch current price from KuCoin.
///
/// Endpoint: `GET https://api.kucoin.com/api/v1/market/stats?symbol={BASE}-USDT`
pub async fn fetch_kucoin(symbol: &str) -> Result<ExternalPrice, OracleError> {
    let base = symbol.split('/').next().unwrap_or(symbol);
    let sym = format!("{base}-USDT");
    let url = format!("https://api.kucoin.com/api/v1/market/stats?symbol={sym}");

    #[derive(Deserialize)]
    struct KuData {
        last: Option<String>,
        vol: Option<String>,
    }
    #[derive(Deserialize)]
    struct KuResponse { data: KuData }

    let resp: KuResponse = http_client()?
        .get(&url)
        .send()
        .await
        .map_err(|e| OracleError::Http(format!("kucoin: {e}")))?
        .json()
        .await
        .map_err(|e| OracleError::Http(format!("kucoin parse: {e}")))?;

    let price_str = resp.data.last
        .ok_or_else(|| OracleError::Http("kucoin: missing last price".into()))?;
    let price_f: f64 = price_str.parse()
        .map_err(|_| OracleError::Http(format!("kucoin: bad price '{price_str}'")))?;
    if price_f <= 0.0 {
        return Err(OracleError::InvalidPrice(0));
    }
    let volume_f: f64 = resp.data.vol.and_then(|v| v.parse().ok()).unwrap_or(180_000.0);

    Ok(ExternalPrice { source: "kucoin", price: Price::from_f64(price_f), volume: volume_f })
}

/// Fetch current price from CoinGecko (aggregator cross-check).
///
/// Endpoint: `GET https://api.coingecko.com/api/v3/simple/price?ids={ID}&vs_currencies=usd`
/// Rate limit: 30 req/min on free tier; use sparingly.
pub async fn fetch_coingecko(symbol: &str) -> Result<ExternalPrice, OracleError> {
    let coingecko_id = symbol_to_coingecko_id(symbol.split('/').next().unwrap_or(symbol))
        .ok_or_else(|| OracleError::Http(format!("coingecko: no ID mapping for '{symbol}'")))?;

    let url = format!(
        "https://api.coingecko.com/api/v3/simple/price?ids={coingecko_id}&vs_currencies=usd"
    );

    let resp: serde_json::Value = http_client()?
        .get(&url)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| OracleError::Http(format!("coingecko: {e}")))?
        .json()
        .await
        .map_err(|e| OracleError::Http(format!("coingecko parse: {e}")))?;

    let price_f = resp
        .get(coingecko_id)
        .and_then(|obj| obj.get("usd"))
        .and_then(|v| v.as_f64())
        .ok_or_else(|| OracleError::Http(format!("coingecko: missing price for '{coingecko_id}'")))?;

    if price_f <= 0.0 {
        return Err(OracleError::InvalidPrice(0));
    }

    Ok(ExternalPrice { source: "coingecko", price: Price::from_f64(price_f), volume: 100_000.0 })
}

/// Map a token symbol to its CoinGecko ID.
fn symbol_to_coingecko_id(symbol: &str) -> Option<&'static str> {
    match symbol.to_uppercase().as_str() {
        "ZBX"  => Some("zebvix-chain"),
        // L-1 fix: ZUSD was mapped to "usd-coin" (USDC) — wrong mapping.
        // ZUSD is ZBX's own stablecoin; it may not be listed on CoinGecko yet.
        // Map to None so the oracle falls back to other sources rather than
        // returning USDC's price (~$1 always, hiding any ZUSD depeg).
        "ZUSD" => None,
        "ETH"  => Some("ethereum"),
        "BTC"  => Some("bitcoin"),
        "BNB"  => Some("binancecoin"),
        "SOL"  => Some("solana"),
        "AVAX" => Some("avalanche-2"),
        "MATIC"=> Some("matic-network"),
        "ARB"  => Some("arbitrum"),
        "OP"   => Some("optimism"),
        "LINK" => Some("chainlink"),
        "DOT"  => Some("polkadot"),
        _      => None,
    }
}

/// Aggregate multiple source prices into a VWAP.
pub fn aggregate_vwap(sources: &[ExternalPrice]) -> Option<Price> {
    if sources.is_empty() { return None; }
    let total_volume: f64 = sources.iter().map(|s| s.volume).sum();
    if total_volume <= 0.0 { return None; }
    let vwap = sources.iter()
        .map(|s| s.price.to_f64() * s.volume)
        .sum::<f64>() / total_volume;
    Some(Price::from_f64(vwap))
}

/// Fetch price for a feed from all sources, return VWAP.
///
/// Tries all configured CEX sources in priority order. Each source failure
/// is silently skipped; the VWAP is computed over whichever sources succeed.
/// Returns `Err(AllSourcesFailed)` only when every source fails.
pub async fn fetch_price_vwap(feed_id: &FeedId) -> Result<Price, OracleError> {
    let symbol = feed_id.0.as_str();
    let base = symbol.split('/').next().unwrap_or(symbol);

    let mut sources = Vec::new();

    // Tier 1: Primary CEX (highest volume, most reliable)
    if let Ok(p) = fetch_binance(base).await  { sources.push(p); }
    if let Ok(p) = fetch_coinbase(base).await { sources.push(p); }
    if let Ok(p) = fetch_kraken(base).await   { sources.push(p); }
    // Tier 2: Secondary CEX (broader coverage)
    if let Ok(p) = fetch_gate(base).await     { sources.push(p); }
    if let Ok(p) = fetch_bybit(base).await    { sources.push(p); }
    if let Ok(p) = fetch_kucoin(base).await   { sources.push(p); }
    // Tier 3: Aggregators (cross-validation, lower weight)
    if let Ok(p) = fetch_coingecko(base).await { sources.push(p); }

    if sources.is_empty() {
        return Err(OracleError::AllSourcesFailed(feed_id.clone()));
    }

    aggregate_vwap(&sources)
        .ok_or_else(|| OracleError::AllSourcesFailed(feed_id.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vwap_weighted_correctly() {
        let sources = vec![
            ExternalPrice { source: "a", price: Price::from_f64(100.0), volume: 1000.0 },
            ExternalPrice { source: "b", price: Price::from_f64(200.0), volume: 1000.0 },
        ];
        let vwap = aggregate_vwap(&sources).unwrap();
        assert!((vwap.to_f64() - 150.0).abs() < 0.01);
    }

    #[test]
    fn vwap_respects_volume_weight() {
        let sources = vec![
            ExternalPrice { source: "a", price: Price::from_f64(100.0), volume: 3000.0 },
            ExternalPrice { source: "b", price: Price::from_f64(200.0), volume: 1000.0 },
        ];
        let vwap = aggregate_vwap(&sources).unwrap();
        assert!((vwap.to_f64() - 125.0).abs() < 0.01);
    }

    #[test]
    fn coingecko_id_mapping() {
        assert_eq!(symbol_to_coingecko_id("ETH"),  Some("ethereum"));
        assert_eq!(symbol_to_coingecko_id("BTC"),  Some("bitcoin"));
        assert_eq!(symbol_to_coingecko_id("ZBX"),  Some("zebvix-chain"));
        assert_eq!(symbol_to_coingecko_id("FAKE"), None);
    }
}
