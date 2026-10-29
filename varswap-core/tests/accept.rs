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
use varswap_core::{accept_print, CoreError, RawPrint, UnixTimestamp};

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
    let (p, run) = accept_print(&s, &good).unwrap();
    assert_eq!((run.first(), run.last()), (1, 1));
    assert_eq!(p.stored().price(), 100_000_000);
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
