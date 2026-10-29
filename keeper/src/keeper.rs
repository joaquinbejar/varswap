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
}

struct View {
    spec: WindowSpec,
    state: WindowState,
    filled: Vec<bool>,
}

impl Keeper {
    /// A keeper for the program at `program_id`.
    #[must_use]
    pub const fn new(program_id: Address) -> Self {
        Self { program_id }
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

        if now <= deadline {
            for k in 0..spec.buckets()? {
                if view.filled.get(k as usize).copied().unwrap_or(true) {
                    continue;
                }
                let b = spec.boundary(k)?.get();
                if b > now {
                    // No print can be canonical for a boundary that has not passed yet.
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
