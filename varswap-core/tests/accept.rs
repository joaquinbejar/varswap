//! `accept_print`, the acceptance rule `sample` and keepers share: a test per rule.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    missing_docs
)]

mod common;

use common::*;
use varswap_core::{accept_print, window_size, CoreError, RawPrint, UnixTimestamp, Window};

const START: i64 = 1_800_000_000;
const H: u32 = 3_600;

fn raw(price: i64, conf: u64, exponent: i32, publish: i64, prev: i64) -> RawPrint {
    RawPrint {
        feed: FEED,
        price,
        conf,
        exponent,
        publish_time: UnixTimestamp::new(publish),
        prev_publish_time: UnixTimestamp::new(prev),
    }
}

#[test]
fn test_accept_print_rules() {
    // Hourly, 6 h guard, max_conf 10%, exponent −8 (common::spec).
    let s = spec(START, H, 4, 6 * H, 24 * H);
    let b1 = START + i64::from(H);
    let good = raw(100_000_000, 1_000_000, -8, b1 + 1, b1 - 1);
    let a = accept_print(&s, &good).unwrap();
    assert_eq!((a.run().first(), a.run().last()), (1, 1));
    assert_eq!(a.print().stored().price(), 100_000_000);
    assert_eq!(a.spec(), &s);
    // Exactly 10% passes, one atom more fails.
    assert!(accept_print(&s, &raw(100_000_000, 10_000_000, -8, b1 + 1, b1 - 1)).is_ok());
    let cases = [
        (
            "other feed",
            RawPrint {
                feed: OTHER_FEED,
                ..good
            },
            CoreError::WrongFeed,
        ),
        (
            "zero price",
            raw(0, 0, -8, b1 + 1, b1 - 1),
            CoreError::NonPositivePrice,
        ),
        (
            "negative price",
            raw(-1, 0, -8, b1 + 1, b1 - 1),
            CoreError::NonPositivePrice,
        ),
        (
            "confidence over 10%",
            raw(100_000_000, 10_000_001, -8, b1 + 1, b1 - 1),
            CoreError::ConfidenceTooWide,
        ),
        (
            "exponent outside i8",
            raw(1, 0, -200, b1 + 1, b1 - 1),
            CoreError::ExponentOutOfRange,
        ),
        (
            "no previous aggregate",
            raw(1, 0, -8, b1 + 1, 0),
            CoreError::NoPreviousPrint,
        ),
        (
            "other exponent",
            raw(1, 0, -7, b1 + 1, b1 - 1),
            CoreError::ExponentMismatch,
        ),
        (
            "same-second copy",
            raw(1, 0, -8, b1 + 5, b1 + 5),
            CoreError::NotOnGrid,
        ),
        (
            "between boundaries",
            raw(1, 0, -8, b1 + 50, b1 + 5),
            CoreError::NotOnGrid,
        ),
    ];
    for (name, r, want) in cases {
        assert_eq!(accept_print(&s, &r).map(|_| ()), Err(want), "{name}");
    }
}

#[test]
fn test_sample_accepted_equals_sample_and_checks_the_spec() {
    let s = spec(START, H, 4, 6 * H, 24 * H);
    let now = UnixTimestamp::new(START + 10 * i64::from(H));
    let prints = [
        raw(100_000_000, 0, -8, START + 1, START - 1),
        raw(
            101_000_000,
            0,
            -8,
            START + 3 * i64::from(H) + 5,
            START + i64::from(H) - 9,
        ),
        raw(
            99_000_000,
            0,
            -8,
            START + 4 * i64::from(H) + 2,
            START + 4 * i64::from(H) - 1,
        ),
    ];
    let mut a_data = vec![0u8; window_size(5).unwrap()];
    let mut b_data = a_data.clone();
    let mut a = Window::init(&mut a_data, s, 0).unwrap();
    let mut b = Window::init(&mut b_data, s, 0).unwrap();
    for r in &prints {
        let acc = accept_print(&s, r).unwrap();
        let ra = a.sample_accepted(&acc, now, &ExactKernel).unwrap();
        let rb = b.sample(acc.print(), now, &ExactKernel).unwrap();
        assert_eq!(ra, rb);
        // Replaying the same accepted print is still rejected.
        assert_eq!(
            a.sample_accepted(&acc, now, &ExactKernel),
            Err(CoreError::BucketAlreadySampled)
        );
    }
    assert_eq!(a_data, b_data);
    // A print accepted for another window's spec is refused.
    let other = spec(START, H, 5, 6 * H, 24 * H);
    let acc = accept_print(&other, &raw(1, 0, -8, START + 1, START - 1)).unwrap();
    let mut data = vec![0u8; window_size(5).unwrap()];
    let mut w = Window::init(&mut data, s, 0).unwrap();
    assert_eq!(
        w.sample_accepted(&acc, now, &ExactKernel),
        Err(CoreError::SpecMismatch)
    );
}
