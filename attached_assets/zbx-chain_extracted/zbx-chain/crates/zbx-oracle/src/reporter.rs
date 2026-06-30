//! Oracle reporter — a node that fetches prices and submits them on-chain.

use crate::{feed::FeedId, feed::Price, error::OracleError};
use serde_big_array::BigArray;
use serde::{Serialize, Deserialize};

/// A single price report from a reporter node.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PriceReport {
    /// Which price feed (e.g. "ZBX/USD")
    pub feed_id:   FeedId,
    /// Price with 8 decimal places
    pub price:     Price,
    /// Unix timestamp when the price was fetched
    pub timestamp: u64,
    /// Reporter's ZBX address (20 bytes)
    pub reporter:  [u8; 20],
    /// ECDSA signature over (feed_id, price, timestamp) — prevents replay
    #[serde(with = "BigArray")]
    pub signature: [u8; 64],
}

impl PriceReport {
    /// Create a new signed price report.
    pub fn new(
        feed_id:   FeedId,
        price:     Price,
        timestamp: u64,
        reporter:  [u8; 20],
        private_key: &[u8; 32],
    ) -> Result<Self, OracleError> {
        if !price.is_valid() {
            return Err(OracleError::InvalidPrice(price.0));
        }
        let msg    = Self::signing_message(&feed_id, price, timestamp);
        let sig    = sign_message(&msg, private_key)?;
        Ok(Self { feed_id, price, timestamp, reporter, signature: sig })
    }

    /// Message to sign: H(feed_id || price || timestamp).
    pub fn signing_message(feed_id: &FeedId, price: Price, timestamp: u64) -> [u8; 32] {
        use sha2::{Sha256, Digest};
        let mut h = Sha256::new();
        h.update(feed_id.0.as_bytes());
        h.update(&price.0.to_le_bytes());
        h.update(&timestamp.to_le_bytes());
        h.finalize().into()
    }

    /// Check if this report has expired (older than max_age_secs).
    pub fn is_expired(&self, now: u64, max_age_secs: u64) -> bool {
        now.saturating_sub(self.timestamp) > max_age_secs
    }

    /// Verify the signature on this report.
    pub fn verify_sig(&self) -> bool {
        let msg = Self::signing_message(&self.feed_id, self.price, self.timestamp);
        verify_signature(&msg, &self.signature, &self.reporter)
    }
}

/// ECDSA signing — uses zbx-crypto secp256k1 (k256, RFC 6979 deterministic, low-S).
fn sign_message(msg: &[u8; 32], key: &[u8; 32]) -> Result<[u8; 64], OracleError> {
    use zbx_crypto::secp256k1::PrivKey;
    use zbx_types::H256;
    let priv_key = PrivKey::from_bytes(key)
        .map_err(|_| OracleError::InvalidSignature)?;
    let full_sig = priv_key.sign(&H256(*msg));
    let sig_bytes = full_sig.to_bytes(); // [u8;65] = r(32) || s(32) || v(1)
    let mut out = [0u8; 64];
    out.copy_from_slice(&sig_bytes[..64]); // store r || s (v stored implicitly; try both on verify)
    Ok(out)
}

/// ECDSA verification — recovers signer address from r||s (both v=0 and v=1)
/// and compares against expected address.
fn verify_signature(msg: &[u8; 32], sig: &[u8; 64], addr: &[u8; 20]) -> bool {
    use zbx_crypto::secp256k1::{Signature, recover_signer};
    use zbx_types::{H256, address::Address};
    let expected = Address(*addr);
    let hash = H256(*msg);
    for v in [0u8, 1u8] {
        let mut full = [0u8; 65];
        full[..64].copy_from_slice(sig);
        full[64] = v;
        if let Ok(parsed_sig) = Signature::from_bytes(&full) {
            if let Ok(recovered) = recover_signer(&hash, &parsed_sig) {
                if recovered == expected {
                    return true;
                }
            }
        }
    }
    false
}

/// The oracle reporter — runs in each oracle node.
///
/// Fetches prices from external sources and submits reports on-chain.
pub struct OracleReporter {
    /// This reporter's ZBX address
    pub address:     [u8; 20],
    /// Private key for signing (kept in memory only)
    private_key:     [u8; 32],
    /// Feeds this reporter covers
    pub feeds:       Vec<FeedId>,
    /// External price fetcher
    pub fetcher_url: String,
}

impl OracleReporter {
    pub fn new(
        address:     [u8; 20],
        private_key: [u8; 32],
        feeds:       Vec<FeedId>,
        fetcher_url: String,
    ) -> Self {
        Self { address, private_key, feeds, fetcher_url }
    }

    /// Fetch current price for a feed from live external market sources.
    ///
    /// Routes each feed to the appropriate fetcher:
    ///   - USD/INR → dedicated forex fetcher (RBI, WazirX, CoinDCX, ExchangeRate-API)
    ///   - All crypto feeds → VWAP across CEX sources (Binance, Coinbase, Kraken,
    ///     Gate, Bybit, KuCoin, CoinGecko)
    ///
    /// Returns `OracleError::AllSourcesFailed` if every configured source fails.
    pub async fn fetch_price(&self, feed_id: &FeedId) -> Result<Price, OracleError> {
        if feed_id == &FeedId::usd_inr() {
            return crate::inr_fetcher::fetch_usd_inr_vwap().await;
        }
        crate::fetcher::fetch_price_vwap(feed_id).await
    }

    /// Produce a signed price report for the given feed.
    pub async fn report(
        &self,
        feed_id:   &FeedId,
        timestamp: u64,
    ) -> Result<PriceReport, OracleError> {
        let price = self.fetch_price(feed_id).await?;
        PriceReport::new(feed_id.clone(), price, timestamp, self.address, &self.private_key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed::FeedId;
    use zbx_crypto::secp256k1::PrivKey;

    /// A small, valid secp256k1 private key (scalar value = 7).
    fn test_key() -> [u8; 32] {
        let mut k = [0u8; 32];
        k[31] = 7;
        k
    }

    /// Derive the [u8; 20] address that corresponds to `key`.
    fn addr_of(key: &[u8; 32]) -> [u8; 20] {
        *PrivKey::from_bytes(key).unwrap().to_address().as_bytes()
    }

    // ── sign_message / verify_signature round-trip tests ─────────────────────

    #[test]
    fn sign_verify_roundtrip() {
        let key  = test_key();
        let addr = addr_of(&key);
        let msg  = [0xabu8; 32];
        let sig  = sign_message(&msg, &key).expect("sign_message should not fail");
        assert!(
            verify_signature(&msg, &sig, &addr),
            "verify_signature must accept the signer's own signature"
        );
    }

    #[test]
    fn verify_rejects_wrong_address() {
        let key   = test_key();
        let msg   = [0x01u8; 32];
        let sig   = sign_message(&msg, &key).expect("sign_message failed");
        let wrong = [0xffu8; 20];
        assert!(!verify_signature(&msg, &sig, &wrong),
            "verify_signature must reject a mismatched address");
    }

    #[test]
    fn verify_rejects_tampered_signature() {
        let key  = test_key();
        let addr = addr_of(&key);
        let msg  = [0x55u8; 32];
        let mut sig = sign_message(&msg, &key).expect("sign_message failed");
        sig[10] ^= 0xff;
        assert!(!verify_signature(&msg, &sig, &addr),
            "verify_signature must reject a corrupted signature");
    }

    #[test]
    fn verify_rejects_wrong_message() {
        let key  = test_key();
        let addr = addr_of(&key);
        let msg1 = [0xaau8; 32];
        let msg2 = [0xbbu8; 32];
        let sig  = sign_message(&msg1, &key).expect("sign_message failed");
        assert!(!verify_signature(&msg2, &sig, &addr),
            "signature over msg1 must not verify against msg2");
    }

    #[test]
    fn verify_rejects_all_zero_signature() {
        let key  = test_key();
        let addr = addr_of(&key);
        let msg  = [0x11u8; 32];
        assert!(!verify_signature(&msg, &[0u8; 64], &addr));
    }

    #[test]
    fn different_keys_produce_different_signatures() {
        let key1: [u8; 32] = { let mut k = [0u8; 32]; k[31] = 7;  k };
        let key2: [u8; 32] = { let mut k = [0u8; 32]; k[31] = 13; k };
        let msg = [0x99u8; 32];
        let s1 = sign_message(&msg, &key1).unwrap();
        let s2 = sign_message(&msg, &key2).unwrap();
        assert_ne!(s1, s2, "different private keys must yield different signatures");
    }

    #[test]
    fn sign_message_is_deterministic() {
        let key = test_key();
        let msg = [0xddu8; 32];
        let s1 = sign_message(&msg, &key).unwrap();
        let s2 = sign_message(&msg, &key).unwrap();
        assert_eq!(s1, s2, "RFC 6979 requires deterministic signatures");
    }

    // ── PriceReport round-trip tests ─────────────────────────────────────────

    #[test]
    fn price_report_new_and_verify_sig() {
        let key  = test_key();
        let addr = addr_of(&key);
        let feed = FeedId("ZBX/USD".to_string());
        let report = PriceReport::new(feed.clone(), 100_000_000, 1_700_000_000, addr, &key)
            .expect("PriceReport::new failed");
        assert!(report.verify_sig(), "PriceReport::verify_sig must pass for a self-signed report");
        assert_eq!(report.feed_id.0, "ZBX/USD");
        assert_eq!(report.price,     100_000_000);
        assert_eq!(report.reporter,  addr);
    }

    #[test]
    fn price_report_verify_fails_after_price_tamper() {
        let key  = test_key();
        let addr = addr_of(&key);
        let feed = FeedId("ETH/USD".to_string());
        let mut report = PriceReport::new(feed, 200_000_000, 1_700_000_000, addr, &key)
            .expect("PriceReport::new failed");
        // Modify price after signing — the stored signature no longer matches
        report.price = 999_999_999;
        assert!(!report.verify_sig(),
            "verify_sig must fail when the price is modified post-signing");
    }

    #[test]
    fn price_report_signing_message_is_deterministic() {
        let feed = FeedId("BTC/USD".to_string());
        let m1 = PriceReport::signing_message(&feed, 50_000_000_000, 1_700_000_000);
        let m2 = PriceReport::signing_message(&feed, 50_000_000_000, 1_700_000_000);
        assert_eq!(m1, m2, "signing_message must be deterministic");
    }

    #[test]
    fn price_report_signing_message_changes_with_price() {
        let feed = FeedId("BTC/USD".to_string());
        let m1 = PriceReport::signing_message(&feed, 50_000_000_000, 1_700_000_000);
        let m2 = PriceReport::signing_message(&feed, 50_000_000_001, 1_700_000_000);
        assert_ne!(m1, m2, "signing_message must differ when price differs");
    }
}