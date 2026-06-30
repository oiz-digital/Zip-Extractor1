//! Integration tests for the ZBX oracle system.

use zbx_oracle::{
    feed::{FeedId, Price, DECIMALS},
    reporter::PriceReport,
    aggregator::OracleAggregator,
    round::{RoundManager, OracleRound},
    error::OracleError,
};

fn reporter(n: u8) -> [u8; 20] { [n; 20] }

fn make_report(feed: FeedId, price_usd: f64, ts: u64, rep: u8) -> PriceReport {
    PriceReport {
        feed_id:   feed,
        price:     Price::from_f64(price_usd),
        timestamp: ts,
        reporter:  reporter(rep),
        signature: [0u8; 64],
    }
}

const NOW: u64 = 1_700_000_000;

// ── Aggregator tests ────────────────────────────────────────────────────────

#[test]
fn aggregator_returns_median() {
    let agg = OracleAggregator::new(3);
    let reports = vec![
        make_report(FeedId::zbx_usd(), 2.48, NOW, 1),
        make_report(FeedId::zbx_usd(), 2.50, NOW, 2),
        make_report(FeedId::zbx_usd(), 2.52, NOW, 3),
    ];
    let r = agg.aggregate(&reports, NOW).unwrap();
    assert!((r.price.to_f64() - 2.50).abs() < 0.001);
}

#[test]
fn aggregator_filters_outliers() {
    let agg = OracleAggregator::new(3);
    let reports = vec![
        make_report(FeedId::zbx_usd(), 2.50, NOW, 1),
        make_report(FeedId::zbx_usd(), 2.51, NOW, 2),
        make_report(FeedId::zbx_usd(), 2.49, NOW, 3),
        make_report(FeedId::zbx_usd(), 9999.99, NOW, 4), // outlier
        make_report(FeedId::zbx_usd(), 0.00001, NOW, 5), // outlier
    ];
    let r = agg.aggregate(&reports, NOW).unwrap();
    // After outlier removal: median of [2.49, 2.50, 2.51] = 2.50
    assert!((r.price.to_f64() - 2.50).abs() < 0.01);
    assert!(r.outliers_removed >= 2);
}

// ── Round manager tests ─────────────────────────────────────────────────────

#[test]
fn round_opens_and_closes() {
    let whitelist = vec![reporter(1), reporter(2), reporter(3)];
    let mut mgr = RoundManager::new(whitelist.clone(), 3);

    let round_id = mgr.new_round(FeedId::zbx_usd(), NOW);
    assert_eq!(round_id, 1);

    for (i, &rep) in [1u8, 2, 3].iter().enumerate() {
        mgr.submit(
            &FeedId::zbx_usd(),
            make_report(FeedId::zbx_usd(), 2.50 + i as f64 * 0.01, NOW, rep),
            NOW,
        ).unwrap();
    }

    let price = mgr.close_round(&FeedId::zbx_usd(), NOW).unwrap();
    assert!((price.to_f64() - 2.51).abs() < 0.01); // median of 2.50, 2.51, 2.52
}

#[test]
fn unauthorized_reporter_rejected() {
    let whitelist = vec![reporter(1)];
    let mut mgr = RoundManager::new(whitelist, 1);
    mgr.new_round(FeedId::zbx_usd(), NOW);

    let report = make_report(FeedId::zbx_usd(), 2.50, NOW, 99); // not whitelisted
    let err = mgr.submit(&FeedId::zbx_usd(), report, NOW).unwrap_err();
    assert!(matches!(err, OracleError::UnauthorizedReporter(_)));
}

#[test]
fn duplicate_reporter_rejected() {
    let whitelist = vec![reporter(1), reporter(2)];
    let mut mgr = RoundManager::new(whitelist, 2);
    mgr.new_round(FeedId::zbx_usd(), NOW);

    mgr.submit(&FeedId::zbx_usd(), make_report(FeedId::zbx_usd(), 2.50, NOW, 1), NOW).unwrap();
    let err = mgr.submit(&FeedId::zbx_usd(), make_report(FeedId::zbx_usd(), 2.51, NOW, 1), NOW).unwrap_err();
    assert!(matches!(err, OracleError::DuplicateReport(_)));
}

#[test]
fn latest_round_data_returns_last_closed() {
    let whitelist = (1..=3).map(reporter).collect();
    let mut mgr = RoundManager::new(whitelist, 3);
    mgr.new_round(FeedId::eth_usd(), NOW);

    for i in 1..=3u8 {
        mgr.submit(
            &FeedId::eth_usd(),
            make_report(FeedId::eth_usd(), 3500.0 + i as f64, NOW, i),
            NOW,
        ).unwrap();
    }
    mgr.close_round(&FeedId::eth_usd(), NOW).unwrap();

    let (round_id, price, ts) = mgr.latest_round_data(&FeedId::eth_usd()).unwrap();
    assert_eq!(round_id, 1);
    assert!((price.to_f64() - 3502.0).abs() < 1.0); // median of 3501, 3502, 3503
    assert_eq!(ts, NOW);
}

// ── Price type tests ────────────────────────────────────────────────────────

#[test]
fn price_from_f64_roundtrip() {
    let p = Price::from_f64(2.50);
    assert!((p.to_f64() - 2.50).abs() < 1e-6);
}

#[test]
fn price_deviation_calculation() {
    let base = Price::from_f64(100.0);
    let high = Price::from_f64(101.0);
    let low  = Price::from_f64(99.0);
    assert!((base.deviation_pct(high) - 1.0).abs() < 0.01);
    assert!((base.deviation_pct(low)  - 1.0).abs() < 0.01);
}

#[test]
fn zusd_peg_check() {
    // ZUSD should be within 0.1% of $1.00
    let zusd_price = Price::from_f64(1.0005); // 0.05% above peg
    let peg        = Price::from_f64(1.00);
    assert!(zusd_price.deviation_pct(peg) < 0.1);
}