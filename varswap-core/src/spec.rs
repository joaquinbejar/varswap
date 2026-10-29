//! The sampling grid and the canonical-print bucket assignment (`docs/PLAN.md` 3.3, 4.1).

use pyth_lite::{FeedId, UnixTimestamp};

use crate::error::CoreError;
use crate::layout::WindowSpecRaw;
use crate::print::Print;
use crate::types::{Address, Bps, Seconds};
use crate::window::MAX_BUCKETS;

/// Largest accepted confidence bound: 100%.
const MAX_CONF_BPS: u16 = 10_000;

/// A validated window: grid `b_k = start + k × interval` for `k` in `[0, N]`, `N >= 1`, plus
/// the risk parameters copied from the feed's `FeedConfig` at creation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowSpec {
    feed: FeedId,
    start: UnixTimestamp,
    end: UnixTimestamp,
    interval: Seconds,
    tolerance: Seconds,
    max_conf: Bps,
    backfill_deadline: Seconds,
    receiver_program: Address,
    /// `N`, cached.
    last: u32,
}

/// The risk parameters a window copies from its `FeedConfig`, never from the creator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RiskParams {
    /// Outage guard (4.1).
    pub tolerance: Seconds,
    /// Confidence sanity bound (4.1).
    pub max_conf: Bps,
    /// Backfill period after `end` (4.1, 4.3).
    pub backfill_deadline: Seconds,
    /// Pyth receiver program id.
    pub receiver_program: Address,
}

/// An inclusive, non-empty range of bucket indices `[first, last]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BucketRun {
    first: u32,
    last: u32,
}

impl BucketRun {
    /// First bucket of the run.
    #[inline]
    #[must_use]
    pub const fn first(self) -> u32 {
        self.first
    }

    /// Last bucket of the run, inclusive.
    #[inline]
    #[must_use]
    pub const fn last(self) -> u32 {
        self.last
    }

    /// Number of buckets, at least 1.
    ///
    /// # Errors
    ///
    /// [`CoreError::Overflow`] (unreachable for runs built by this crate).
    #[inline]
    pub fn count(self) -> Result<u32, CoreError> {
        self.last
            .checked_sub(self.first)
            .and_then(|d| d.checked_add(1))
            .ok_or(CoreError::Overflow)
    }
}

impl WindowSpec {
    /// Validates a window: `start < end`, `interval > 0`, `(end − start) % interval == 0`,
    /// `N + 1 <= MAX_BUCKETS`, `tolerance > 0`, `backfill_deadline > 0`,
    /// `0 < max_conf <= 10,000` bps, and `end + backfill_deadline` representable.
    ///
    /// # Errors
    ///
    /// The [`CoreError`] variant of the first rule that fails.
    pub fn try_new(
        feed: FeedId,
        start: UnixTimestamp,
        end: UnixTimestamp,
        interval: Seconds,
        risk: RiskParams,
    ) -> Result<Self, CoreError> {
        if start >= end {
            return Err(CoreError::EmptyWindow);
        }
        if interval.get() == 0 {
            return Err(CoreError::ZeroInterval);
        }
        let len = i128::from(end.get())
            .checked_sub(i128::from(start.get()))
            .ok_or(CoreError::Overflow)?;
        let iv = i128::from(interval.get());
        if len.checked_rem_euclid(iv).ok_or(CoreError::Overflow)? != 0 {
            return Err(CoreError::EndOffGrid);
        }
        let last = len.checked_div_euclid(iv).ok_or(CoreError::Overflow)?;
        let last = u32::try_from(last).map_err(|_| CoreError::TooManyBuckets)?;
        if last >= MAX_BUCKETS {
            return Err(CoreError::TooManyBuckets);
        }
        if risk.tolerance.get() == 0 {
            return Err(CoreError::ZeroTolerance);
        }
        if risk.backfill_deadline.get() == 0 {
            return Err(CoreError::ZeroBackfillDeadline);
        }
        if risk.max_conf.get() == 0 || risk.max_conf.get() > MAX_CONF_BPS {
            return Err(CoreError::InvalidMaxConf);
        }
        end.get()
            .checked_add(i64::from(risk.backfill_deadline.get()))
            .ok_or(CoreError::Overflow)?;
        Ok(Self {
            feed,
            start,
            end,
            interval,
            tolerance: risk.tolerance,
            max_conf: risk.max_conf,
            backfill_deadline: risk.backfill_deadline,
            receiver_program: risk.receiver_program,
            last,
        })
    }

    /// `create_window` rules that need the clock and the feed: `start > now` (no window whose
    /// sampling already began) and `N + 1 <= max_buckets` (the feed's own cap).
    ///
    /// # Errors
    ///
    /// [`CoreError::StartNotInFuture`] or [`CoreError::TooManyBuckets`].
    #[inline]
    pub fn check_creatable(&self, now: UnixTimestamp, max_buckets: u32) -> Result<(), CoreError> {
        if self.start <= now {
            return Err(CoreError::StartNotInFuture);
        }
        if self.buckets()? > max_buckets {
            return Err(CoreError::TooManyBuckets);
        }
        Ok(())
    }

    /// Feed id.
    #[inline]
    #[must_use]
    pub const fn feed(&self) -> FeedId {
        self.feed
    }

    /// `b_0`.
    #[inline]
    #[must_use]
    pub const fn start(&self) -> UnixTimestamp {
        self.start
    }

    /// `b_N`.
    #[inline]
    #[must_use]
    pub const fn end(&self) -> UnixTimestamp {
        self.end
    }

    /// Grid spacing.
    #[inline]
    #[must_use]
    pub const fn interval(&self) -> Seconds {
        self.interval
    }

    /// Outage guard.
    #[inline]
    #[must_use]
    pub const fn tolerance(&self) -> Seconds {
        self.tolerance
    }

    /// Confidence sanity bound, passed to `pyth_lite::PriceRequirements`.
    #[inline]
    #[must_use]
    pub const fn max_conf(&self) -> Bps {
        self.max_conf
    }

    /// Backfill period after `end`.
    #[inline]
    #[must_use]
    pub const fn backfill_deadline(&self) -> Seconds {
        self.backfill_deadline
    }

    /// Pyth receiver program id.
    #[inline]
    #[must_use]
    pub const fn receiver_program(&self) -> Address {
        self.receiver_program
    }

    /// `N`, the index of the last bucket.
    #[inline]
    #[must_use]
    pub const fn last_index(&self) -> u32 {
        self.last
    }

    /// `B = N + 1`, the number of buckets.
    ///
    /// # Errors
    ///
    /// [`CoreError::Overflow`] (unreachable: `N < MAX_BUCKETS`).
    #[inline]
    pub fn buckets(&self) -> Result<u32, CoreError> {
        self.last.checked_add(1).ok_or(CoreError::Overflow)
    }

    /// `end − start` in seconds, the annualization span.
    ///
    /// # Errors
    ///
    /// [`CoreError::Overflow`] (unreachable: `start < end`).
    #[inline]
    pub fn span_seconds(&self) -> Result<u64, CoreError> {
        let d = self
            .end
            .get()
            .checked_sub(self.start.get())
            .ok_or(CoreError::Overflow)?;
        u64::try_from(d).map_err(|_| CoreError::Overflow)
    }

    /// `end + backfill_deadline`: the last second at which a sample is accepted.
    ///
    /// # Errors
    ///
    /// [`CoreError::Overflow`] (checked at construction).
    #[inline]
    pub fn deadline(&self) -> Result<UnixTimestamp, CoreError> {
        self.end
            .get()
            .checked_add(i64::from(self.backfill_deadline.get()))
            .map(UnixTimestamp::new)
            .ok_or(CoreError::Overflow)
    }

    /// `b_k = start + k × interval`.
    ///
    /// # Errors
    ///
    /// [`CoreError::NotOnGrid`] if `k > N`, [`CoreError::Overflow`].
    pub fn boundary(&self, k: u32) -> Result<UnixTimestamp, CoreError> {
        if k > self.last {
            return Err(CoreError::NotOnGrid);
        }
        i64::from(k)
            .checked_mul(i64::from(self.interval.get()))
            .and_then(|off| self.start.get().checked_add(off))
            .map(UnixTimestamp::new)
            .ok_or(CoreError::Overflow)
    }

    /// The buckets a print is canonical for: every `k` in `[0, N]` with
    /// `prev_publish_time < b_k <= publish_time <= b_k + tolerance` (4.1).
    ///
    /// `k_lo = max(0, floor((prev − start) / interval) + 1, ceil((publish − tolerance −
    /// start) / interval))`, `k_hi = min(N, floor((publish − start) / interval))`, with floor
    /// and ceiling by Euclidean division (`prev − start` is negative for the print of `b_0`;
    /// truncating division would be off by one there). Exact: no rounding beyond the floors
    /// and ceilings that define the grid. At most `tolerance / interval + 1` buckets.
    ///
    /// # Errors
    ///
    /// [`CoreError::NotOnGrid`] if no bucket qualifies (including `prev >= publish`, a
    /// failed-slot or same-second copy), [`CoreError::Overflow`].
    pub fn canonical_run(&self, print: &Print) -> Result<BucketRun, CoreError> {
        let start = i128::from(self.start.get());
        let iv = i128::from(self.interval.get());
        let prev = i128::from(print.prev_publish_time().get());
        let publish = i128::from(print.publish_time().get());
        let tol = i128::from(self.tolerance.get());
        let floor = |a: i128| a.checked_div_euclid(iv).ok_or(CoreError::Overflow);
        let sub = |a: i128, b: i128| a.checked_sub(b).ok_or(CoreError::Overflow);

        let hi = floor(sub(publish, start)?)?.min(i128::from(self.last));
        let after_prev = floor(sub(prev, start)?)?
            .checked_add(1)
            .ok_or(CoreError::Overflow)?;
        // ceil(a / iv) = floor((a + iv − 1) / iv) for iv > 0, any sign of a.
        let within_tol = floor(
            sub(sub(publish, tol)?, start)?
                .checked_add(iv)
                .and_then(|a| a.checked_sub(1))
                .ok_or(CoreError::Overflow)?,
        )?;
        let lo = after_prev.max(within_tol).max(0);
        if lo > hi {
            return Err(CoreError::NotOnGrid);
        }
        Ok(BucketRun {
            first: u32::try_from(lo).map_err(|_| CoreError::Overflow)?,
            last: u32::try_from(hi).map_err(|_| CoreError::Overflow)?,
        })
    }

    /// Stored form.
    #[must_use]
    pub fn to_raw(&self) -> WindowSpecRaw {
        WindowSpecRaw {
            feed: self.feed.to_bytes(),
            start: self.start.get(),
            end: self.end.get(),
            receiver_program: self.receiver_program,
            interval: self.interval.get(),
            tolerance: self.tolerance.get(),
            backfill_deadline: self.backfill_deadline.get(),
            max_conf: self.max_conf.get(),
            _pad: [0; 2],
        }
    }

    /// Validates a stored spec with the same rules as [`WindowSpec::try_new`].
    ///
    /// # Errors
    ///
    /// [`CoreError::InvalidAccountData`] if the padding is not zero, else as
    /// [`WindowSpec::try_new`].
    pub fn from_raw(raw: &WindowSpecRaw) -> Result<Self, CoreError> {
        if raw._pad != [0; 2] {
            return Err(CoreError::InvalidAccountData);
        }
        Self::try_new(
            FeedId::new(raw.feed),
            UnixTimestamp::new(raw.start),
            UnixTimestamp::new(raw.end),
            Seconds::new(raw.interval),
            RiskParams {
                tolerance: Seconds::new(raw.tolerance),
                max_conf: Bps::new(raw.max_conf),
                backfill_deadline: Seconds::new(raw.backfill_deadline),
                receiver_program: raw.receiver_program,
            },
        )
    }
}
