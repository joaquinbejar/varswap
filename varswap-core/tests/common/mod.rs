//! Shared test helpers: an exact integer return kernel and spec builders.

#![allow(dead_code)]

use std::num::NonZeroU64;

use varswap_core::{Bps, FeedId, Print, RiskParams, Seconds, UnixTimestamp, WindowSpec};

pub const FEED: FeedId = FeedId::new([7; 32]);
pub const OTHER_FEED: FeedId = FeedId::new([8; 32]);
pub const RECEIVER: [u8; 32] = [9; 32];

/// The tests' kernel is the crate's exact integer kernel; the alias keeps the test names.
pub use varswap_core::IntegerKernel as ExactKernel;

pub fn risk(tolerance: u32, backfill: u32) -> RiskParams {
    RiskParams {
        tolerance: Seconds::new(tolerance),
        max_conf: Bps::new(1_000),
        backfill_deadline: Seconds::new(backfill),
        receiver_program: RECEIVER,
    }
}

pub fn spec(start: i64, interval: u32, n: u32, tolerance: u32, backfill: u32) -> WindowSpec {
    WindowSpec::try_new(
        FEED,
        UnixTimestamp::new(start),
        UnixTimestamp::new(start + i64::from(interval) * i64::from(n)),
        Seconds::new(interval),
        risk(tolerance, backfill),
    )
    .unwrap()
}

pub fn print(price: u64, exponent: i32, publish: i64, prev: i64) -> Print {
    Print::try_new(
        FEED,
        NonZeroU64::new(price).unwrap(),
        exponent,
        UnixTimestamp::new(publish),
        UnixTimestamp::new(prev),
    )
    .unwrap()
}

pub fn t(s: i64) -> UnixTimestamp {
    UnixTimestamp::new(s)
}
