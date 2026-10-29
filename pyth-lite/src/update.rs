//! Decoding of `PriceUpdateV2` and the price checks.

use core::num::NonZeroU64;

use crate::error::PythError;
use crate::ids::{FeedId, PRICE_UPDATE_V2_DISCRIMINATOR};

/// Basis points in one unit: a confidence bound of `max_conf_bps` means
/// `conf / price <= max_conf_bps / 10_000`.
const BPS: u128 = 10_000;

/// Seconds since the Unix epoch, as in Pyth `publish_time` and the Solana `Clock`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct UnixTimestamp(i64);

impl UnixTimestamp {
    /// Wraps a Unix time in seconds.
    #[inline]
    #[must_use]
    pub const fn new(seconds: i64) -> Self {
        Self(seconds)
    }

    /// The Unix time in seconds.
    #[inline]
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }
}

/// How many Wormhole guardian signatures the receiver checked when the update was posted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerificationLevel {
    /// A subset of the guardian set signed (Borsh tag 0, then the count).
    Partial {
        /// Number of signatures checked.
        num_signatures: u8,
    },
    /// The full guardian quorum signed (Borsh tag 1).
    Full,
}

/// The Pyth `PriceFeedMessage` carried by the update, as stored (no checks applied).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PriceMessage {
    feed_id: FeedId,
    price: i64,
    conf: u64,
    exponent: i32,
    publish_time: UnixTimestamp,
    prev_publish_time: UnixTimestamp,
    ema_price: i64,
    ema_conf: u64,
}

impl PriceMessage {
    /// Feed the price belongs to.
    #[inline]
    #[must_use]
    pub const fn feed_id(&self) -> FeedId {
        self.feed_id
    }

    /// Price mantissa: the price is `price × 10^exponent`.
    #[inline]
    #[must_use]
    pub const fn price(&self) -> i64 {
        self.price
    }

    /// Confidence interval mantissa, same exponent as the price.
    #[inline]
    #[must_use]
    pub const fn conf(&self) -> u64 {
        self.conf
    }

    /// Decimal exponent of `price`, `conf`, `ema_price` and `ema_conf`.
    #[inline]
    #[must_use]
    pub const fn exponent(&self) -> i32 {
        self.exponent
    }

    /// Time the aggregate was published, in whole seconds.
    #[inline]
    #[must_use]
    pub const fn publish_time(&self) -> UnixTimestamp {
        self.publish_time
    }

    /// `publish_time` of the previous aggregate of this feed, in whole seconds.
    #[inline]
    #[must_use]
    pub const fn prev_publish_time(&self) -> UnixTimestamp {
        self.prev_publish_time
    }

    /// Exponentially weighted moving average price mantissa.
    #[inline]
    #[must_use]
    pub const fn ema_price(&self) -> i64 {
        self.ema_price
    }

    /// Exponentially weighted moving average confidence mantissa.
    #[inline]
    #[must_use]
    pub const fn ema_conf(&self) -> u64 {
        self.ema_conf
    }
}

/// A decoded `PriceUpdateV2` account whose owner and discriminator were checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PriceUpdate {
    write_authority: [u8; 32],
    verification_level: VerificationLevel,
    message: PriceMessage,
    posted_slot: u64,
}

/// What [`PriceUpdate::verify`] requires of an update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PriceRequirements {
    feed_id: FeedId,
    max_age_secs: Option<u64>,
    max_conf_bps: u16,
}

impl PriceRequirements {
    /// `feed_id`: the only feed accepted. `max_age_secs`: the staleness bound in seconds, or
    /// `None` for no bound (a caller that accepts late canonical prints, as varswap's
    /// `sample` does, bounds time itself). `max_conf_bps`: the largest accepted
    /// `conf / price`, in basis points.
    #[inline]
    #[must_use]
    pub const fn new(feed_id: FeedId, max_age_secs: Option<u64>, max_conf_bps: u16) -> Self {
        Self {
            feed_id,
            max_age_secs,
            max_conf_bps,
        }
    }
}

/// A price that passed every check of [`PriceUpdate::verify`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OraclePrice {
    feed_id: FeedId,
    price: NonZeroU64,
    conf: u64,
    exponent: i32,
    publish_time: UnixTimestamp,
    prev_publish_time: UnixTimestamp,
}

impl OraclePrice {
    /// Feed the price belongs to.
    #[inline]
    #[must_use]
    pub const fn feed_id(&self) -> FeedId {
        self.feed_id
    }

    /// Price mantissa, positive: the price is `price × 10^exponent`.
    #[inline]
    #[must_use]
    pub const fn price(&self) -> NonZeroU64 {
        self.price
    }

    /// Confidence interval mantissa, same exponent as the price.
    #[inline]
    #[must_use]
    pub const fn conf(&self) -> u64 {
        self.conf
    }

    /// Decimal exponent of `price` and `conf`.
    #[inline]
    #[must_use]
    pub const fn exponent(&self) -> i32 {
        self.exponent
    }

    /// Time the aggregate was published, in whole seconds.
    #[inline]
    #[must_use]
    pub const fn publish_time(&self) -> UnixTimestamp {
        self.publish_time
    }

    /// `publish_time` of the previous aggregate of this feed, in whole seconds.
    #[inline]
    #[must_use]
    pub const fn prev_publish_time(&self) -> UnixTimestamp {
        self.prev_publish_time
    }
}

impl PriceUpdate {
    /// Checks that `owner` is `receiver` (normally [`crate::RECEIVER_PROGRAM_ID`]), then
    /// decodes `data` as with [`PriceUpdate::decode`].
    ///
    /// # Errors
    ///
    /// [`PythError::WrongOwner`], or any error of [`PriceUpdate::decode`].
    #[inline]
    pub fn load(owner: &[u8; 32], data: &[u8], receiver: &[u8; 32]) -> Result<Self, PythError> {
        if owner != receiver {
            return Err(PythError::WrongOwner);
        }
        Self::decode(data)
    }

    /// Decodes account data: discriminator, then the Borsh layout by computed offsets.
    /// Bytes after `posted_slot` are ignored, as Anchor does (a `Full` update leaves one).
    /// The owner is not checked here: use [`PriceUpdate::load`] on untrusted accounts.
    ///
    /// # Errors
    ///
    /// [`PythError::WrongDiscriminator`], [`PythError::UnknownVerificationLevel`], or
    /// [`PythError::Truncated`] if `data` ends before `posted_slot`.
    pub fn decode(data: &[u8]) -> Result<Self, PythError> {
        let mut r = Reader(data);
        if r.array::<8>()? != PRICE_UPDATE_V2_DISCRIMINATOR {
            return Err(PythError::WrongDiscriminator);
        }
        let write_authority = r.array::<32>()?;
        let verification_level = match r.array::<1>()? {
            [0] => VerificationLevel::Partial {
                num_signatures: u8::from_le_bytes(r.array()?),
            },
            [1] => VerificationLevel::Full,
            _ => return Err(PythError::UnknownVerificationLevel),
        };
        let message = PriceMessage {
            feed_id: FeedId::new(r.array()?),
            price: i64::from_le_bytes(r.array()?),
            conf: u64::from_le_bytes(r.array()?),
            exponent: i32::from_le_bytes(r.array()?),
            publish_time: UnixTimestamp::new(i64::from_le_bytes(r.array()?)),
            prev_publish_time: UnixTimestamp::new(i64::from_le_bytes(r.array()?)),
            ema_price: i64::from_le_bytes(r.array()?),
            ema_conf: u64::from_le_bytes(r.array()?),
        };
        let posted_slot = u64::from_le_bytes(r.array()?);
        Ok(Self {
            write_authority,
            verification_level,
            message,
            posted_slot,
        })
    }

    /// Runs the price checks in this order: `Full` verification, feed id, positive price,
    /// staleness, confidence.
    ///
    /// Staleness follows pyth-solana-receiver-sdk's `get_price_no_older_than`: fresh when
    /// `now − publish_time <= max_age`, so a `publish_time` ahead of `now` (clock skew
    /// between Pythnet and Solana) is fresh. Confidence: `conf × 10_000 <= price ×
    /// max_conf_bps`, exact in `u128` (no rounding).
    ///
    /// # Errors
    ///
    /// [`PythError::NotFullyVerified`], [`PythError::WrongFeed`],
    /// [`PythError::NonPositivePrice`], [`PythError::Stale`] or
    /// [`PythError::ConfidenceTooWide`], the first that applies.
    pub fn verify(
        &self,
        req: &PriceRequirements,
        now: UnixTimestamp,
    ) -> Result<OraclePrice, PythError> {
        if self.verification_level != VerificationLevel::Full {
            return Err(PythError::NotFullyVerified);
        }
        let m = &self.message;
        if m.feed_id != req.feed_id {
            return Err(PythError::WrongFeed);
        }
        let price = u64::try_from(m.price)
            .ok()
            .and_then(NonZeroU64::new)
            .ok_or(PythError::NonPositivePrice)?;
        if let Some(max_age) = req.max_age_secs {
            // i64 − i64 always fits in i128; checked only because the lint set requires it.
            let age = i128::from(now.get())
                .checked_sub(i128::from(m.publish_time.get()))
                .ok_or(PythError::ArithmeticOverflow)?;
            if age > i128::from(max_age) {
                return Err(PythError::Stale);
            }
        }
        // u64 × 10_000 and u64 × u16 both fit in u128.
        let lhs = u128::from(m.conf)
            .checked_mul(BPS)
            .ok_or(PythError::ArithmeticOverflow)?;
        let rhs = u128::from(price.get())
            .checked_mul(u128::from(req.max_conf_bps))
            .ok_or(PythError::ArithmeticOverflow)?;
        if lhs > rhs {
            return Err(PythError::ConfidenceTooWide);
        }
        Ok(OraclePrice {
            feed_id: m.feed_id,
            price,
            conf: m.conf,
            exponent: m.exponent,
            publish_time: m.publish_time,
            prev_publish_time: m.prev_publish_time,
        })
    }

    /// Write authority recorded by the receiver: the account allowed to overwrite or close
    /// this update.
    #[inline]
    #[must_use]
    pub const fn write_authority(&self) -> [u8; 32] {
        self.write_authority
    }

    /// Verification level recorded by the receiver.
    #[inline]
    #[must_use]
    pub const fn verification_level(&self) -> VerificationLevel {
        self.verification_level
    }

    /// The price message, unchecked.
    #[inline]
    #[must_use]
    pub const fn message(&self) -> &PriceMessage {
        &self.message
    }

    /// Solana slot at which the update was posted.
    #[inline]
    #[must_use]
    pub const fn posted_slot(&self) -> u64 {
        self.posted_slot
    }
}

/// Forward-only reader over account data; running out is [`PythError::Truncated`].
struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    #[inline]
    fn array<const N: usize>(&mut self) -> Result<[u8; N], PythError> {
        let (head, rest) = self
            .0
            .split_first_chunk::<N>()
            .ok_or(PythError::Truncated)?;
        self.0 = rest;
        Ok(*head)
    }
}
