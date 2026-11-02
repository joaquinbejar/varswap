//! The checks behind `varswap-watch` (`docs/PLAN-improvements.md`, I6): pure functions over
//! what the chain shows, so they are tested without a cluster, and the alert book that decides
//! when an alert fires again.

use std::collections::HashMap;

use serde::Serialize;
use varswap_core::{WindowSpec, WindowState};

use crate::KeeperError;

/// What a check found wrong.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Alert {
    /// `lag`, `deadline`, `unfinalized`, `degraded`, `balance` or `encoded-vaa`.
    pub kind: &'static str,
    /// The window, payer or encoded VAA address it is about.
    pub subject: String,
    /// One line for a human.
    pub message: String,
}

/// One window as the watcher sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WindowStatus {
    /// `pending`, `sampling`, `finalized` or `degraded`.
    pub state: &'static str,
    /// Filled buckets.
    pub filled: u32,
    /// Buckets.
    pub buckets: u32,
    /// Empty buckets whose boundary has passed.
    pub missing_due: Vec<u32>,
    /// Seconds since the oldest empty due boundary (0 if none).
    pub max_lag_s: i64,
    /// Seconds until `end + backfill_deadline` (negative once past).
    pub to_deadline_s: i64,
}

/// Alert thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Thresholds {
    /// A due bucket may stay empty this long before `lag` fires.
    pub grace_s: i64,
    /// With buckets still empty, `deadline` fires this long before the backfill deadline.
    pub deadline_warning_s: i64,
}

fn state_name(s: WindowState) -> &'static str {
    match s {
        WindowState::Pending => "pending",
        WindowState::Sampling => "sampling",
        WindowState::Finalized => "finalized",
        WindowState::Degraded => "degraded",
    }
}

/// Assesses one window at cluster time `now` from its spec, state and filled bitmap.
///
/// # Errors
///
/// `KeeperError::Core` if the spec's grid or deadline cannot be computed (a corrupt spec), or
/// `KeeperError::Config` on a time overflow.
pub fn assess_window(
    window: &str,
    now: i64,
    spec: &WindowSpec,
    state: WindowState,
    filled: &[bool],
    t: Thresholds,
) -> Result<(WindowStatus, Vec<Alert>), KeeperError> {
    let buckets = spec.buckets()?;
    let deadline = spec.deadline()?.get();
    let to_deadline_s = deadline
        .checked_sub(now)
        .ok_or(KeeperError::Config("time out of range"))?;
    let mut missing_due = Vec::new();
    let mut missing_total: u32 = 0;
    let mut oldest_due: Option<i64> = None;
    for k in 0..buckets {
        if filled.get(k as usize).copied().unwrap_or(false) {
            continue;
        }
        missing_total = missing_total.saturating_add(1);
        let b = spec.boundary(k)?.get();
        if b <= now {
            missing_due.push(k);
            oldest_due = Some(oldest_due.map_or(b, |o| o.min(b)));
        }
    }
    let max_lag_s = oldest_due.map_or(Ok(0), |b| {
        now.checked_sub(b)
            .ok_or(KeeperError::Config("time out of range"))
    })?;
    let filled_count = buckets.saturating_sub(missing_total);

    let mut alerts = Vec::new();
    let subject = window.to_string();
    match state {
        WindowState::Degraded => alerts.push(Alert {
            kind: "degraded",
            subject: subject.clone(),
            message: format!(
                "window {window} degraded with {filled_count} of {buckets} buckets: its swaps unwind"
            ),
        }),
        WindowState::Finalized => {}
        WindowState::Pending | WindowState::Sampling => {
            if max_lag_s > t.grace_s {
                let first = missing_due.first().copied().unwrap_or(0);
                alerts.push(Alert {
                    kind: "lag",
                    subject: subject.clone(),
                    message: format!(
                        "window {window}: {} due bucket(s) empty, the oldest (bucket {first}) for {max_lag_s} s (grace {} s): is the keeper running?",
                        missing_due.len(),
                        t.grace_s
                    ),
                });
            }
            if missing_total > 0 && to_deadline_s >= 0 && to_deadline_s <= t.deadline_warning_s {
                alerts.push(Alert {
                    kind: "deadline",
                    subject: subject.clone(),
                    message: format!(
                        "window {window}: {missing_total} bucket(s) empty with {to_deadline_s} s to the backfill deadline: it degrades if they stay empty"
                    ),
                });
            }
            // Past the deadline, or complete for longer than the grace after `end`: anyone can
            // finalize it, and a keeper would have.
            let end = spec.end().get();
            let complete_since_grace = missing_total == 0
                && now > end.checked_add(t.grace_s).ok_or(KeeperError::Config("time out of range"))?;
            if to_deadline_s < 0 || complete_since_grace {
                alerts.push(Alert {
                    kind: "unfinalized",
                    subject,
                    message: format!("window {window} can be finalized and is not"),
                });
            }
        }
    }
    Ok((
        WindowStatus {
            state: state_name(state),
            filled: filled_count,
            buckets,
            missing_due,
            max_lag_s,
            to_deadline_s,
        },
        alerts,
    ))
}

/// Encoded VAAs a payer has open, with the block height each was first seen at, to tell a
/// post in flight (seconds) from a leftover the keeper has not closed (its scan runs every 12
/// ticks: up to an hour under the agent).
#[derive(Debug, Default)]
pub struct VaaTracker {
    first_seen: HashMap<String, u64>,
}

impl VaaTracker {
    /// Records the open accounts seen at `height` and returns an alert for each one open for
    /// more than `max_blocks`. Accounts no longer open are forgotten.
    pub fn observe(
        &mut self,
        payer: &str,
        open: &[String],
        height: u64,
        max_blocks: u64,
    ) -> Vec<Alert> {
        self.first_seen.retain(|a, _| open.contains(a));
        let mut alerts = Vec::new();
        for a in open {
            let first = *self.first_seen.entry(a.clone()).or_insert(height);
            let age = height.saturating_sub(first);
            if age > max_blocks {
                alerts.push(Alert {
                    kind: "encoded-vaa",
                    subject: a.clone(),
                    message: format!(
                        "encoded VAA {a} of payer {payer} open for {age} blocks: the keeper's scan should have closed it"
                    ),
                });
            }
        }
        alerts
    }
}

/// When an alert fires: the first time it is seen, then again every `realert_s` while it
/// stays active. An alert that stops being reported is resolved.
#[derive(Debug, Default)]
pub struct AlertBook {
    last_fired: HashMap<(&'static str, String), i64>,
}

impl AlertBook {
    /// Given every alert active now, returns those to fire and the keys resolved since the
    /// last call.
    pub fn update(
        &mut self,
        active: &[Alert],
        now: i64,
        realert_s: i64,
    ) -> (Vec<Alert>, Vec<(&'static str, String)>) {
        let keys: Vec<(&'static str, String)> =
            active.iter().map(|a| (a.kind, a.subject.clone())).collect();
        let resolved: Vec<_> = self
            .last_fired
            .keys()
            .filter(|k| !keys.contains(k))
            .cloned()
            .collect();
        for k in &resolved {
            self.last_fired.remove(k);
        }
        let mut fire = Vec::new();
        for a in active {
            let key = (a.kind, a.subject.clone());
            let due = self
                .last_fired
                .get(&key)
                .is_none_or(|last| now.saturating_sub(*last) >= realert_s);
            if due {
                self.last_fired.insert(key, now);
                fire.push(a.clone());
            }
        }
        (fire, resolved)
    }
}
