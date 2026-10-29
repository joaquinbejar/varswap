//! Errors returned by the decoder and the price checks.

use core::fmt;

/// Why a `PriceUpdateV2` account was rejected. Programs map it into their own error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PythError {
    /// The account is not owned by the expected Pyth receiver program.
    WrongOwner = 0,
    /// The first 8 bytes are not the `PriceUpdateV2` Anchor discriminator.
    WrongDiscriminator = 1,
    /// The data ends before the last field of the layout.
    Truncated = 2,
    /// The `verification_level` tag is neither `Partial` (0) nor `Full` (1).
    UnknownVerificationLevel = 3,
    /// The update was posted with `Partial` verification; only `Full` is accepted.
    NotFullyVerified = 4,
    /// The update is for another feed.
    WrongFeed = 5,
    /// The price is zero or negative.
    NonPositivePrice = 6,
    /// `publish_time` is older than `now − max_age`.
    Stale = 7,
    /// `conf / price` exceeds the configured bound.
    ConfidenceTooWide = 8,
    /// A checked arithmetic step overflowed.
    ArithmeticOverflow = 9,
}

impl fmt::Display for PythError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::WrongOwner => "price update not owned by the Pyth receiver",
            Self::WrongDiscriminator => "not a PriceUpdateV2 account",
            Self::Truncated => "price update data truncated",
            Self::UnknownVerificationLevel => "unknown verification level tag",
            Self::NotFullyVerified => "price update not fully verified",
            Self::WrongFeed => "price update for another feed",
            Self::NonPositivePrice => "price is not positive",
            Self::Stale => "price update is stale",
            Self::ConfidenceTooWide => "confidence interval too wide",
            Self::ArithmeticOverflow => "arithmetic overflow",
        };
        f.write_str(s)
    }
}
