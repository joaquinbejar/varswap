//! Shared test helpers: an exact integer return kernel and spec builders.

#![allow(dead_code)]

use std::num::NonZeroU64;

use varswap_core::{
    Bps, CoreError, FeedId, Print, ReturnKernel, RiskParams, Seconds, StoredPrice, SumSq,
    UnixTimestamp, WindowSpec, YEAR_SECONDS,
};

pub const FEED: FeedId = FeedId::new([7; 32]);
pub const OTHER_FEED: FeedId = FeedId::new([8; 32]);
pub const RECEIVER: [u8; 32] = [9; 32];

/// Exact integer stand-in for `ln(to / from)²`: `(to − from)²` with both prices rescaled to
/// the smaller exponent. Not a log return, but exact, so any accumulation error shows up as
/// an inequality, never as a tolerance.
pub struct ExactKernel;

pub fn value(p: StoredPrice, exp: i8) -> i128 {
    let shift = u32::try_from(i32::from(p.exponent()) - i32::from(exp)).unwrap();
    i128::from(p.price()) * 10i128.pow(shift)
}

impl ReturnKernel for ExactKernel {
    fn squared_log_return(&self, from: StoredPrice, to: StoredPrice) -> Result<SumSq, CoreError> {
        let e = from.exponent().min(to.exponent());
        let d = value(to, e) - value(from, e);
        Ok(SumSq::from_raw(u128::try_from(d * d).unwrap()))
    }

    fn annualize(&self, sum_sq: SumSq, span_seconds: u64) -> Result<u128, CoreError> {
        Ok(sum_sq.raw() * u128::from(YEAR_SECONDS) / u128::from(span_seconds))
    }
}

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
