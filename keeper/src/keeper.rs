//! One keeper step for one window.

use varswap_core::{accept_print, RawPrint, UnixTimestamp, WindowSpec, WindowState};
use varswap_sdk::{decode, Address};

use crate::{Chain, KeeperError, PriceSource, PrintMsg};

/// What one [`Keeper::tick`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TickReport {
    /// Canonical prints posted (each fills one bucket, or a run of them after an outage).
    pub posted: u32,
    /// Posts that failed because another keeper had already filled the bucket (benign).
    pub already_filled: u32,
    /// Canonical prints the window's own rules reject (exponent, confidence, price, no
    /// previous aggregate): the program would reject them too, so their buckets stay empty.
    pub skipped: u32,
    /// The state `finalize` left the window in, if this tick called it.
    pub finalized: Option<WindowState>,
    /// Whether the window was already final (nothing to do).
    pub done: bool,
}

/// The sampling keeper for one program deployment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Keeper {
    program_id: Address,
    /// A standby keeper samples only buckets whose boundary is at least this many seconds old
    /// (`docs/PLAN-improvements.md`, I6); 0 for a primary.
    lag: u32,
}

/// The smallest standby lag: a standby never races a healthy primary's first tick.
pub const MIN_STANDBY_LAG: u32 = 600;
/// A standby lag must leave at least this much of the interval before the next boundary.
pub const STANDBY_LAG_MARGIN: u32 = 300;

struct View {
    spec: WindowSpec,
    state: WindowState,
    filled: Vec<bool>,
}

impl Keeper {
    /// A keeper for the program at `program_id`.
    #[must_use]
    pub const fn new(program_id: Address) -> Self {
        Self { program_id, lag: 0 }
    }

    /// A standby keeper: it samples a bucket only once its boundary is `lag` seconds old, so
    /// while a primary keeps up it finds nothing to do and queries no price source.
    #[must_use]
    pub const fn standby(program_id: Address, lag: u32) -> Self {
        Self { program_id, lag }
    }

    /// The standby lag (0 for a primary).
    #[must_use]
    pub const fn lag(&self) -> u32 {
        self.lag
    }

    /// Checks a standby lag against a window's interval: at least [`MIN_STANDBY_LAG`] and at
    /// most `interval − STANDBY_LAG_MARGIN` (for an hourly window: 600 to 3,300 s).
    ///
    /// # Errors
    ///
    /// `KeeperError::Config` if the lag is outside those bounds for this interval.
    pub fn check_standby_lag(lag: u32, interval: u32) -> Result<(), KeeperError> {
        let max = interval.checked_sub(STANDBY_LAG_MARGIN);
        match max {
            Some(max) if (MIN_STANDBY_LAG..=max).contains(&lag) => Ok(()),
            _ => Err(KeeperError::Config(
                "standby lag must be at least 600 s and at most the window interval minus 300 s",
            )),
        }
    }

    /// Reads a window's spec (for checks before the first tick).
    ///
    /// # Errors
    ///
    /// As a tick's read: the account is missing or not a window of this program.
    pub fn window_spec<C: Chain>(
        &self,
        chain: &C,
        window: &Address,
    ) -> Result<WindowSpec, KeeperError> {
        Ok(self.read(chain, window)?.spec)
    }

    fn read<C: Chain>(&self, chain: &C, window: &Address) -> Result<View, KeeperError> {
        let (owner, data) = chain
            .account(window)?
            .ok_or(KeeperError::Decode("window account missing"))?;
        let info = decode::window(&self.program_id, &owner, &data)
            .map_err(|_| KeeperError::Decode("not a valid window"))?;
        let filled = decode::filled_buckets(&self.program_id, &owner, &data)
            .map_err(|_| KeeperError::Decode("not a valid window"))?;
        Ok(View {
            spec: info.spec,
            state: info.state,
            filled,
        })
    }

    /// Posts every missing canonical print that is already published and that the window
    /// accepts, then finalizes when the window is complete or past its backfill deadline.
    /// Idempotent: a second call with nothing new posts nothing.
    ///
    /// # Errors
    ///
    /// Source, chain and decoding errors. A failed post is benign only if the window, re-read
    /// right after, already holds that bucket; otherwise the tick returns the error **without
    /// finalizing**.
    pub fn tick<C: Chain, S: PriceSource>(
        &self,
        chain: &mut C,
        source: &S,
        window: &Address,
    ) -> Result<TickReport, KeeperError> {
        let mut report = TickReport::default();
        let now = chain.now()?;
        let mut view = self.read(chain, window)?;
        if view.state.is_final() {
            report.done = true;
            return Ok(report);
        }
        let spec = view.spec;
        let feed = spec.feed().to_bytes();
        let deadline = spec.deadline()?.get();
        let tolerance = i64::from(spec.tolerance().get());
        // The newest boundary this keeper may sample (a standby lags behind the primary).
        let horizon = now
            .checked_sub(i64::from(self.lag))
            .ok_or(KeeperError::Config("clock out of range"))?;

        if now <= deadline {
            for k in 0..spec.buckets()? {
                if view.filled.get(k as usize).copied().unwrap_or(true) {
                    continue;
                }
                let b = spec.boundary(k)?.get();
                if b > horizon {
                    // No print can be canonical for a boundary that has not passed yet; a
                    // standby also leaves boundaries younger than its lag to the primary.
                    break;
                }
                let Some(m) = source.canonical_at(&feed, b, tolerance, now)? else {
                    continue;
                };
                let Some(run) = accepted(&spec, &m) else {
                    report.skipped = report.skipped.saturating_add(1);
                    continue;
                };
                match chain.post_sample(window, &feed, &m) {
                    Ok(()) => report.posted = report.posted.saturating_add(1),
                    Err(e) => {
                        view = self.read(chain, window)?;
                        if view.filled.get(k as usize).copied() == Some(true) {
                            report.already_filled = report.already_filled.saturating_add(1);
                            continue;
                        }
                        return Err(e);
                    }
                }
                // The print fills its whole run (an outage's boundaries share it).
                for j in run.0..=run.1 {
                    if let Some(f) = view.filled.get_mut(j as usize) {
                        *f = true;
                    }
                }
            }
        }

        let complete = view.filled.iter().all(|f| *f);
        if complete || now > deadline {
            chain.finalize(window)?;
            report.finalized = Some(self.read(chain, window)?.state);
        }
        Ok(report)
    }
}

/// The bucket run `print` fills if the window accepts it: `varswap_core::accept_print`, the
/// very function the program's `sample` applies, so the keeper skips exactly what the program
/// would reject.
fn accepted(spec: &WindowSpec, m: &PrintMsg) -> Option<(u32, u32)> {
    let raw = RawPrint {
        feed: spec.feed(),
        price: m.price,
        conf: m.conf,
        exponent: m.expo,
        publish_time: UnixTimestamp::new(m.publish_time),
        prev_publish_time: UnixTimestamp::new(m.prev_publish_time),
    };
    accept_print(spec, &raw)
        .ok()
        .map(|a| (a.run().first(), a.run().last()))
}
