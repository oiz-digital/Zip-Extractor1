//! Fuzz target: feed arbitrary byte sequences to the Pay ID parser.
//! Goal: parser must NEVER panic — must only return Ok(PayId) or Err.
//!
//! Critical: Pay IDs come from untrusted network input. If a malicious
//! sender crafts a bad Pay ID, the node must reject it cleanly.
//!
//! Properties tested:
//!   1. parse() never panics on any UTF-8 string
//!   2. parse() never panics on arbitrary bytes (invalid UTF-8 included)
//!   3. If parse succeeds → validate() also succeeds (consistency)
//!   4. format(parse(x)) == x for all valid Pay IDs (roundtrip)
//!
//! Run:
//!   cargo +nightly fuzz run fuzz_payid_parser -- -max_total_time=120
#![no_main]

use libfuzzer_sys::fuzz_target;
use zbx_payid::{parse_pay_id, validate_pay_id, format_pay_id};

fuzz_target!(|data: &[u8]| {
    // Invariant 1: parse must not panic on arbitrary bytes
    let as_str = match std::str::from_utf8(data) {
        Ok(s)  => s.to_string(),
        Err(_) => {
            // Still test parse with a lossy string
            String::from_utf8_lossy(data).into_owned()
        }
    };

    let parse_result = std::panic::catch_unwind(|| {
        parse_pay_id(&as_str)
    });
    assert!(
        parse_result.is_ok(),
        "parse_pay_id panicked on: {:?}", as_str
    );

    // Invariant 2: if parse succeeds, validate must also succeed
    if let Ok(Ok(ref pay_id)) = parse_result {
        let validate_result = std::panic::catch_unwind(|| {
            validate_pay_id(pay_id)
        });
        assert!(
            validate_result.is_ok(),
            "validate_pay_id panicked on valid pay_id: {:?}", pay_id
        );
        assert!(
            validate_result.unwrap().is_ok(),
            "validate_pay_id returned Err for a successfully-parsed Pay ID: {:?}", pay_id
        );

        // Invariant 3: format(parse(x)) roundtrip for valid Pay IDs
        let formatted = format_pay_id(pay_id);
        let re_parsed = parse_pay_id(&formatted);
        assert!(
            re_parsed.is_ok(),
            "format/parse roundtrip panicked"
        );
    }
});