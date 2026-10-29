//! A verified Pyth print as the accumulator consumes it, and a bucket's stored price.

use core::num::NonZeroU64;

use pyth_lite::{FeedId, OraclePrice, UnixTimestamp};

use crate::error::CoreError;

/// A bucket's price as stored in the window: `price × 10^exponent`, `price > 0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoredPrice {
    price: i64,
    exponent: i8,
}

impl StoredPrice {
    /// Validates a stored pair: `price > 0`.
    ///
    /// # Errors
    ///
    /// [`CoreError::InvalidAccountData`] if `price <= 0`.
    #[inline]
    pub fn try_new(price: i64, exponent: i8) -> Result<Self, CoreError> {
        if price <= 0 {
            return Err(CoreError::InvalidAccountData);
        }
        Ok(Self { price, exponent })
    }

    /// Price mantissa, positive.
    #[inline]
    #[must_use]
    pub const fn price(self) -> i64 {
        self.price
    }

    /// Decimal exponent.
    #[inline]
    #[must_use]
    pub const fn exponent(self) -> i8 {
        self.exponent
    }
}

/// A print that may be sampled: positive price, exponent within `i8`, a previous aggregate.
/// Built from a [`pyth_lite::OraclePrice`], which already passed `Full` verification, the feed
/// id and the confidence bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Print {
    feed_id: FeedId,
    stored: StoredPrice,
    publish_time: UnixTimestamp,
    prev_publish_time: UnixTimestamp,
}

impl Print {
    /// Validates the fields of a print.
    ///
    /// # Errors
    ///
    /// [`CoreError::PriceOutOfRange`] if `price > i64::MAX`, [`CoreError::ExponentOutOfRange`]
    /// if the exponent does not fit in an `i8`, [`CoreError::NoPreviousPrint`] if
    /// `prev_publish_time == 0` (`docs/PLAN.md` 4.1).
    pub fn try_new(
        feed_id: FeedId,
        price: NonZeroU64,
        exponent: i32,
        publish_time: UnixTimestamp,
        prev_publish_time: UnixTimestamp,
    ) -> Result<Self, CoreError> {
        let price = i64::try_from(price.get()).map_err(|_| CoreError::PriceOutOfRange)?;
        let exponent = i8::try_from(exponent).map_err(|_| CoreError::ExponentOutOfRange)?;
        if prev_publish_time.get() == 0 {
            return Err(CoreError::NoPreviousPrint);
        }
        Ok(Self {
            feed_id,
            stored: StoredPrice { price, exponent },
            publish_time,
            prev_publish_time,
        })
    }

    /// The feed of the print.
    #[inline]
    #[must_use]
    pub const fn feed_id(&self) -> FeedId {
        self.feed_id
    }

    /// The price and exponent the bucket stores.
    #[inline]
    #[must_use]
    pub const fn stored(&self) -> StoredPrice {
        self.stored
    }

    /// Publish time, whole seconds.
    #[inline]
    #[must_use]
    pub const fn publish_time(&self) -> UnixTimestamp {
        self.publish_time
    }

    /// Publish time of the previous successful aggregate, whole seconds.
    #[inline]
    #[must_use]
    pub const fn prev_publish_time(&self) -> UnixTimestamp {
        self.prev_publish_time
    }
}

impl TryFrom<&OraclePrice> for Print {
    type Error = CoreError;

    #[inline]
    fn try_from(p: &OraclePrice) -> Result<Self, CoreError> {
        Self::try_new(
            p.feed_id(),
            p.price(),
            p.exponent(),
            p.publish_time(),
            p.prev_publish_time(),
        )
    }
}
