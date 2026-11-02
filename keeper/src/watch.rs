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
    /// `lag`, `deadline`, `unfinalized`, `degraded`, `balance`, `encoded-vaa`, `unreadable` or
    /// `rpc`.
    pub kind: &'static str,
    /// The observation that produced it (`window:<address>`, `balance:<payer>`,
    /// `vaas:<payer>`, `rpc`): an alert resolves only when its scope was observed again and the
    /// alert was not reported.
    pub scope: String,
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

/// The scope of a window's checks.
#[must_use]
pub fn window_scope(window: &str) -> String {
    format!("window:{window}")
}

/// The scope of a payer's balance check.
#[must_use]
pub fn balance_scope(payer: &str) -> String {
    format!("balance:{payer}")
}

/// The scope of a payer's encoded VAA scan.
#[must_use]
pub fn vaa_scope(payer: &str) -> String {
    format!("vaas:{payer}")
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
    let scope = window_scope(window);
    match state {
        WindowState::Degraded => alerts.push(Alert {
            kind: "degraded",
            scope: scope.clone(),
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
                    scope: scope.clone(),
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
                    scope: scope.clone(),
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
                    scope,
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
    /// By payer, then by encoded VAA: the block height it was first seen open at. Each payer's
    /// accounts are tracked apart, so observing one payer never forgets another's.
    first_seen: HashMap<String, HashMap<String, u64>>,
}

impl VaaTracker {
    /// Records `payer`'s open accounts seen at `height` (its complete scan) and returns an
    /// alert for each one open for more than `max_blocks`. That payer's accounts no longer open
    /// are forgotten; other payers' are untouched.
    pub fn observe(
        &mut self,
        payer: &str,
        open: &[String],
        height: u64,
        max_blocks: u64,
    ) -> Vec<Alert> {
        let seen = self.first_seen.entry(payer.to_owned()).or_default();
        seen.retain(|a, _| open.contains(a));
        let mut alerts = Vec::new();
        for a in open {
            let first = *seen.entry(a.clone()).or_insert(height);
            let age = height.saturating_sub(first);
            if age > max_blocks {
                alerts.push(Alert {
                    kind: "encoded-vaa",
                    scope: vaa_scope(payer),
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

/// When an alert is delivered: as soon as it is seen, then again every `realert_s` while it
/// stays active; a delivery that fails is retried at the next pass, not after `realert_s`. An
/// alert resolves only when its scope was observed again and the alert was not reported, so a
/// failed read never resolves what it could not see. An active alert whose scope was not
/// observed (a failed read, or a whole pass that could not reach the cluster) is still due
/// for delivery, with its last known payload marked as such.
#[derive(Debug, Default)]
pub struct AlertBook {
    /// Active alerts: the last known payload, and when it was last delivered (`None`: not
    /// yet, or the last attempt failed).
    active: HashMap<(&'static str, String), (Alert, Option<i64>)>,
}

impl AlertBook {
    /// Records the alerts reported now and the scopes observed now. Returns the alerts due for
    /// delivery (reported ones, and retained ones whose scope was not observed, marked last
    /// known) and the alerts resolved (their scope was observed and they were not reported).
    pub fn update(
        &mut self,
        reported: &[Alert],
        observed: &[String],
        now: i64,
        realert_s: i64,
    ) -> (Vec<Alert>, Vec<(&'static str, String)>) {
        let keys: Vec<(&'static str, String)> = reported
            .iter()
            .map(|a| (a.kind, a.subject.clone()))
            .collect();
        let resolved: Vec<_> = self
            .active
            .iter()
            .filter(|(k, (a, _))| !keys.contains(k) && observed.contains(&a.scope))
            .map(|(k, _)| k.clone())
            .collect();
        for k in &resolved {
            self.active.remove(k);
        }
        for a in reported {
            self.active
                .entry((a.kind, a.subject.clone()))
                .and_modify(|e| e.0 = a.clone())
                .or_insert_with(|| (a.clone(), None));
        }
        let mut due = Vec::new();
        for (k, (a, last)) in &self.active {
            let is_due = last.is_none_or(|l| now.saturating_sub(l) >= realert_s);
            if !is_due {
                continue;
            }
            if keys.contains(k) {
                due.push(a.clone());
            } else {
                let mut stale = a.clone();
                stale.message = format!("{} (last known: not observed this pass)", a.message);
                due.push(stale);
            }
        }
        due.sort_by(|x, y| (x.kind, &x.subject).cmp(&(y.kind, &y.subject)));
        (due, resolved)
    }

    /// Records a successful delivery of `a` at `now`; a failed one is simply not recorded.
    pub fn delivered(&mut self, a: &Alert, now: i64) {
        if let Some(e) = self.active.get_mut(&(a.kind, a.subject.clone())) {
            e.1 = Some(now);
        }
    }
}
