//! Errors of the accumulator. The program maps them into its own `#[repr(u32)]` codes.

use core::fmt;

/// Why a window spec, a print, a sample or a finalize was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CoreError {
    /// `start >= end`.
    EmptyWindow = 0,
    /// `interval == 0`.
    ZeroInterval = 1,
    /// `end − start` is not a multiple of `interval`.
    EndOffGrid = 2,
    /// `N + 1` exceeds [`crate::MAX_BUCKETS`] or the feed's `max_buckets`.
    TooManyBuckets = 3,
    /// `tolerance == 0`.
    ZeroTolerance = 4,
    /// `backfill_deadline == 0`.
    ZeroBackfillDeadline = 5,
    /// `max_conf` is 0 or above 10,000 bps.
    InvalidMaxConf = 6,
    /// `start` is not after the creation time.
    StartNotInFuture = 7,
    /// The Pyth exponent does not fit in an `i8`.
    ExponentOutOfRange = 8,
    /// The price mantissa does not fit in an `i64`.
    PriceOutOfRange = 9,
    /// `prev_publish_time == 0`: the first aggregate after an oracle `init_price`, whose
    /// interval overlaps earlier prints (4.1).
    NoPreviousPrint = 10,
    /// The print is for another feed.
    WrongFeed = 11,
    /// The print is canonical for no bucket of the grid.
    NotOnGrid = 12,
    /// A bucket of the print's run is already filled.
    BucketAlreadySampled = 13,
    /// `now > end + backfill_deadline`: no more samples.
    BackfillClosed = 14,
    /// The window is already `Finalized` or `Degraded`.
    WindowFinal = 15,
    /// Some bucket is empty and `now <= end + backfill_deadline`.
    FinalizeTooEarly = 16,
    /// The account bytes are not a valid window (size, tag, version, state, counters).
    InvalidAccountData = 17,
    /// A checked arithmetic step overflowed.
    Overflow = 18,
    /// The return kernel failed; the payload is its own error code.
    Kernel(u8) = 19,
    /// The print's exponent is not the one the window pinned (`docs/PLAN.md` 4.4).
    ExponentMismatch = 20,
}

impl CoreError {
    /// Stable numeric code: the discriminant; [`CoreError::Kernel`] is 19 whatever its
    /// payload (read the payload with a match).
    #[inline]
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::EmptyWindow => 0,
            Self::ZeroInterval => 1,
            Self::EndOffGrid => 2,
            Self::TooManyBuckets => 3,
            Self::ZeroTolerance => 4,
            Self::ZeroBackfillDeadline => 5,
            Self::InvalidMaxConf => 6,
            Self::StartNotInFuture => 7,
            Self::ExponentOutOfRange => 8,
            Self::PriceOutOfRange => 9,
            Self::NoPreviousPrint => 10,
            Self::WrongFeed => 11,
            Self::NotOnGrid => 12,
            Self::BucketAlreadySampled => 13,
            Self::BackfillClosed => 14,
            Self::WindowFinal => 15,
            Self::FinalizeTooEarly => 16,
            Self::InvalidAccountData => 17,
            Self::Overflow => 18,
            Self::Kernel(_) => 19,
            Self::ExponentMismatch => 20,
        }
    }
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::EmptyWindow => "window start is not before its end",
            Self::ZeroInterval => "sampling interval is zero",
            Self::EndOffGrid => "window length is not a multiple of the interval",
            Self::TooManyBuckets => "too many buckets",
            Self::ZeroTolerance => "outage tolerance is zero",
            Self::ZeroBackfillDeadline => "backfill deadline is zero",
            Self::InvalidMaxConf => "confidence bound outside (0, 10000] bps",
            Self::StartNotInFuture => "window start is not in the future",
            Self::ExponentOutOfRange => "price exponent does not fit in i8",
            Self::PriceOutOfRange => "price does not fit in i64",
            Self::NoPreviousPrint => "print has no previous aggregate",
            Self::WrongFeed => "print is for another feed",
            Self::NotOnGrid => "print is canonical for no bucket",
            Self::BucketAlreadySampled => "bucket already sampled",
            Self::BackfillClosed => "backfill deadline passed",
            Self::WindowFinal => "window already final",
            Self::FinalizeTooEarly => "window cannot be finalized yet",
            Self::InvalidAccountData => "invalid window account data",
            Self::Overflow => "arithmetic overflow",
            Self::Kernel(_) => "return kernel error",
            Self::ExponentMismatch => "print exponent differs from the feed's",
        };
        f.write_str(s)
    }
}
