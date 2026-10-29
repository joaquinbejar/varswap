//! Where the keeper gets Pyth prints.

use crate::KeeperError;

/// One Pyth `PriceFeedMessage`, as the keeper needs it to post a `PriceUpdateV2`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PrintMsg {
    /// Price mantissa.
    pub price: i64,
    /// Confidence mantissa.
    pub conf: u64,
    /// Decimal exponent.
    pub expo: i32,
    /// Publish time, seconds.
    pub publish_time: i64,
    /// Previous successful aggregate's publish time, seconds.
    pub prev_publish_time: i64,
}

/// A source of Pyth prints for one feed. Every query returns only prints already published
/// (`publish_time <= now`), and at most the one canonical print the query defines.
pub trait PriceSource {
    /// The canonical print of boundary `t`: `0 < prev_publish_time < t <= publish_time <=
    /// t + tolerance`.
    ///
    /// # Errors
    ///
    /// `KeeperError::Source` if the source cannot answer.
    fn canonical_at(
        &self,
        feed: &[u8; 32],
        t: i64,
        tolerance: i64,
        now: i64,
    ) -> Result<Option<PrintMsg>, KeeperError>;
}

/// A recorded tape of one feed's messages, in publication order: synthetic in tests, or a
/// capture replayed later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TapeSource {
    feed: [u8; 32],
    messages: Vec<PrintMsg>,
}

impl TapeSource {
    /// A tape of `messages` for `feed`.
    #[must_use]
    pub fn new(feed: [u8; 32], messages: Vec<PrintMsg>) -> Self {
        Self { feed, messages }
    }
}

impl PriceSource for TapeSource {
    fn canonical_at(
        &self,
        feed: &[u8; 32],
        t: i64,
        tolerance: i64,
        now: i64,
    ) -> Result<Option<PrintMsg>, KeeperError> {
        if feed != &self.feed {
            return Ok(None);
        }
        let latest = t
            .checked_add(tolerance)
            .ok_or(KeeperError::Config("tolerance overflow"))?;
        Ok(self
            .messages
            .iter()
            .filter(|m| m.publish_time <= now)
            .find(|m| {
                m.prev_publish_time > 0
                    && m.prev_publish_time < t
                    && t <= m.publish_time
                    && m.publish_time <= latest
            })
            .copied())
    }
}
