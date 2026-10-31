// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs

//! The fee cap against a big-integer reference, over the whole input space.
//!
//! The contract computes `floor(total * max_fee_bps / 10000)` without ever
//! forming the product, which would overflow an `i128` near the largest
//! amount. The reference here forms the product exactly in 256 bits and
//! divides it by long division, so the two share no arithmetic. Both must
//! agree on every `total` in `[1, I128_MAX]` and every rate in
//! `[0, MAX_FEE_BPS]`, never fail, and never give more than 1 percent.

extern crate std;

use crate::payout::fee_cap;
use mandate_config::MAX_FEE_BPS;
use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};

/// At least the 100,000 cases the fee cap issue asks for, more when
/// `UJIRA_FEE_CAP_CASES` says so.
fn cases() -> u32 {
    std::env::var("UJIRA_FEE_CAP_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .map_or(100_000, |n: u32| n.max(100_000))
}

/// `floor(total * bps / 10000)` in exact 256-bit arithmetic, for
/// `total >= 0`. The product is built in three 64-bit limbs, most
/// significant first, then divided limb by limb.
fn reference(total: i128, bps: u32) -> i128 {
    let t = u128::try_from(total).expect("a negative total has no fee cap");
    let bps = u128::from(bps);
    let low = |v: u128| v & u128::from(u64::MAX);
    // total = a * 2^64 + b, so total * bps = (a * bps) * 2^64 + b * bps.
    let x = low(t) * bps;
    let y = (t >> 64) * bps;
    let mid = low(y) + (x >> 64);
    let limbs = [(y >> 64) + (mid >> 64), low(mid), low(x)];
    let mut rem: u128 = 0;
    let mut quotient = [0u128; 3];
    for (i, limb) in limbs.iter().enumerate() {
        let cur = (rem << 64) | limb;
        quotient[i] = cur / 10_000;
        rem = cur % 10_000;
    }
    // The product is below 2^134, so the quotient is below 2^121 and its
    // top limb is always 0.
    assert_eq!(quotient[0], 0);
    i128::try_from((quotient[1] << 64) | quotient[2]).unwrap()
}

/// Totals spread over every magnitude, not only the huge ones a uniform
/// draw from `[1, I128_MAX]` gives almost every time.
fn totals() -> impl Strategy<Value = i128> {
    prop_oneof![
        1..=i128::MAX,
        (0u32..127).prop_flat_map(|bits| {
            let lo = 1i128 << bits;
            let hi = if bits == 126 {
                i128::MAX
            } else {
                (1i128 << (bits + 1)) - 1
            };
            lo..=hi
        }),
        (i128::MAX - 1_000_000)..=i128::MAX,
        1i128..=1_000_000,
    ]
}

fn runner() -> TestRunner {
    TestRunner::new(Config {
        cases: cases(),
        failure_persistence: None,
        ..Config::default()
    })
}

#[test]
fn fee_cap_property_matches_the_big_integer_reference() {
    let mut runner = runner();
    let result = runner.run(&(totals(), 0..=MAX_FEE_BPS), |(total, bps)| {
        let got = fee_cap(total, bps);
        prop_assert_eq!(
            got,
            Ok(reference(total, bps)),
            "total {} bps {}",
            total,
            bps
        );
        Ok(())
    });
    if let Err(e) = result {
        panic!("{e}");
    }
}

#[test]
fn fee_cap_property_never_exceeds_one_percent() {
    let mut runner = runner();
    let result = runner.run(&(totals(), 0..=MAX_FEE_BPS), |(total, bps)| {
        let cap = fee_cap(total, bps).map_err(|e| TestCaseError::fail(std::format!("{e:?}")))?;
        prop_assert!(cap >= 0);
        prop_assert!(
            cap <= total / 100,
            "total {} bps {} cap {}",
            total,
            bps,
            cap
        );
        Ok(())
    });
    if let Err(e) = result {
        panic!("{e}");
    }
}

/// Splitting one payout into several never raises the most fee the parts
/// may carry together, so rounding always favours the principal.
#[test]
fn fee_cap_property_splitting_never_raises_the_fee() {
    let mut runner = runner();
    let parts = proptest::collection::vec(1i128..=(i128::MAX / 16), 1..=16);
    let result = runner.run(&(parts, 0..=MAX_FEE_BPS), |(parts, bps)| {
        let whole: i128 = parts.iter().sum();
        let mut split = 0i128;
        for p in &parts {
            split += fee_cap(*p, bps).unwrap();
        }
        prop_assert!(split <= fee_cap(whole, bps).unwrap());
        Ok(())
    });
    if let Err(e) = result {
        panic!("{e}");
    }
}

/// Every entry of the shared edge fixture, against the contract and the
/// reference both, so the fixture itself is checked too.
#[test]
fn fee_cap_property_matches_the_shared_edge_fixture() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../spec/fixtures/fee-cap-edges.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
    let cases = doc["cases"].as_array().unwrap();
    let mut seen_totals = std::collections::BTreeSet::new();
    let mut seen_bps = std::collections::BTreeSet::new();
    for case in cases {
        let total: i128 = case["total"].as_str().unwrap().parse().unwrap();
        let bps = u32::try_from(case["max_fee_bps"].as_u64().unwrap()).unwrap();
        let want: i128 = case["fee_cap"].as_str().unwrap().parse().unwrap();
        assert_eq!(
            fee_cap(total, bps),
            Ok(want),
            "fixture total {total} bps {bps}"
        );
        assert_eq!(
            reference(total, bps),
            want,
            "reference total {total} bps {bps}"
        );
        seen_totals.insert(total);
        seen_bps.insert(bps);
    }
    // The edges the fee cap issue names are all there.
    for t in [1, 9_999, 10_000, 10_001, i128::MAX - 1, i128::MAX] {
        assert!(seen_totals.contains(&t), "fixture misses total {t}");
    }
    for b in [0, 1, 99, 100] {
        assert!(seen_bps.contains(&b), "fixture misses bps {b}");
    }
    assert_eq!(cases.len(), seen_totals.len() * seen_bps.len());
}

/// The reference itself, on values worked out by hand.
#[test]
fn fee_cap_property_reference_is_right_by_hand() {
    assert_eq!(reference(10_000, 100), 100);
    assert_eq!(reference(9_999, 100), 99);
    assert_eq!(reference(10_001, 1), 1);
    assert_eq!(reference(9_999, 1), 0);
    // I128_MAX = 17014118346046923173168730371588410 * 10000 + 5727.
    assert_eq!(
        reference(i128::MAX, 100),
        1_701_411_834_604_692_317_316_873_037_158_841_057
    );
    assert_eq!(
        reference(i128::MAX, 99),
        1_684_397_716_258_645_394_143_704_306_787_252_646
    );
}
