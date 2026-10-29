//! The window's acceptance rule for one print, shared by the program's `sample` and by
//! off-chain keepers, so both decide exactly alike (`docs/PLAN.md` 3.3, 4.1).

use core::num::NonZeroU64;

use pyth_lite::{FeedId, UnixTimestamp};

use crate::error::CoreError;
use crate::print::Print;
use crate::spec::{BucketRun, WindowSpec};

/// Basis points in one unit.
const BPS: u128 = 10_000;

/// The fields of a Pyth price message the acceptance rule reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawPrint {
    /// Feed of the message.
    pub feed: FeedId,
    /// Price mantissa.
    pub price: i64,
    /// Confidence mantissa.
    pub conf: u64,
    /// Decimal exponent.
    pub exponent: i32,
    /// Publish time, whole seconds.
    pub publish_time: UnixTimestamp,
    /// Previous successful aggregate's publish time.
    pub prev_publish_time: UnixTimestamp,
}

/// A print a window accepted, with the run it fills; only [`accept_print`] builds one, for
/// [`crate::Window::sample_accepted`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Accepted {
    spec: WindowSpec,
    print: Print,
    run: BucketRun,
}

impl Accepted {
    /// The window spec it was accepted for.
    #[inline]
    #[must_use]
    pub const fn spec(&self) -> &WindowSpec {
        &self.spec
    }

    /// The print.
    #[inline]
    #[must_use]
    pub const fn print(&self) -> &Print {
        &self.print
    }

    /// The bucket run it fills.
    #[inline]
    #[must_use]
    pub const fn run(&self) -> BucketRun {
        self.run
    }
}

/// Whether `window` accepts `raw` as a canonical print, and the bucket run it fills: the
/// window's feed, a positive price, `conf × 10,000 <= price × max_conf` (exact in `u128`), the
/// [`Print`] rules (exponent within `i8`, price within `i64`, a previous aggregate), the
/// window's pinned exponent, and a non-empty canonical run. It does not look at which buckets
/// are filled (that is [`crate::Window::sample`]'s state) or at the Pyth account's owner and
/// verification level (`pyth-lite`).
///
/// # Errors
///
/// The first of [`CoreError::WrongFeed`], [`CoreError::NonPositivePrice`],
/// [`CoreError::ConfidenceTooWide`], the [`Print::try_new`] errors,
/// [`CoreError::ExponentMismatch`] and [`CoreError::NotOnGrid`] that applies.
pub fn accept_print(window: &WindowSpec, raw: &RawPrint) -> Result<Accepted, CoreError> {
    if raw.feed != window.feed() {
        return Err(CoreError::WrongFeed);
    }
    let price = u64::try_from(raw.price)
        .ok()
        .and_then(NonZeroU64::new)
        .ok_or(CoreError::NonPositivePrice)?;
    let lhs = u128::from(raw.conf)
        .checked_mul(BPS)
        .ok_or(CoreError::Overflow)?;
    let rhs = u128::from(price.get())
        .checked_mul(u128::from(window.max_conf().get()))
        .ok_or(CoreError::Overflow)?;
    if lhs > rhs {
        return Err(CoreError::ConfidenceTooWide);
    }
    let print = Print::try_new(
        raw.feed,
        price,
        raw.exponent,
        raw.publish_time,
        raw.prev_publish_time,
    )?;
    if print.stored().exponent() != window.exponent() {
        return Err(CoreError::ExponentMismatch);
    }
    let run = window.canonical_run(&print)?;
    Ok(Accepted {
        spec: *window,
        print,
        run,
    })
}
