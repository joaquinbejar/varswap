//! `varswap-watch`'s checks on synthetic windows, the alert book and the open VAA tracker
//! (`docs/PLAN-improvements.md`, I6), and the standby lag bounds.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    missing_docs
)]

use varswap_core::{Bps, FeedId, RiskParams, Seconds, UnixTimestamp, WindowSpec, WindowState};
use varswap_keeper::watch::{assess_window, Alert, AlertBook, Thresholds, VaaTracker};
use varswap_keeper::Keeper;

const START: i64 = 1_800_000_000;
const H: u32 = 3_600;

/// 24 hourly intervals (25 buckets), 6 h guard, 24 h backfill.
fn spec() -> WindowSpec {
    WindowSpec::try_new(
        FeedId::new([1; 32]),
        UnixTimestamp::new(START),
        UnixTimestamp::new(START + 24 * i64::from(H)),
        Seconds::new(H),
        RiskParams {
            tolerance: Seconds::new(6 * H),
            max_conf: Bps::new(1_000),
            backfill_deadline: Seconds::new(24 * H),
            receiver_program: [9; 32],
            exponent: -8,
        },
    )
    .unwrap()
}

const T: Thresholds = Thresholds {
    grace_s: 600,
    deadline_warning_s: 6 * 3_600,
};

fn kinds(a: &[Alert]) -> Vec<&'static str> {
    a.iter().map(|a| a.kind).collect()
}

#[test]
fn test_on_time_window_raises_nothing() {
    // b_0..b_3 passed and filled, 5 minutes after b_3.
    let mut filled = vec![false; 25];
    filled[..4].fill(true);
    let now = START + 3 * i64::from(H) + 300;
    let (s, alerts) = assess_window("w", now, &spec(), WindowState::Sampling, &filled, T).unwrap();
    assert!(alerts.is_empty(), "{alerts:?}");
    assert_eq!((s.filled, s.buckets, s.max_lag_s), (4, 25, 0));
    assert!(s.missing_due.is_empty());
}

#[test]
fn test_lag_fires_after_the_grace_only() {
    // b_3 empty: 600 s after it is still within the grace, 601 s is not.
    let mut filled = vec![false; 25];
    filled[..3].fill(true);
    let b3 = START + 3 * i64::from(H);
    let (_, a) = assess_window("w", b3 + 600, &spec(), WindowState::Sampling, &filled, T).unwrap();
    assert!(a.is_empty());
    let (s, a) = assess_window("w", b3 + 601, &spec(), WindowState::Sampling, &filled, T).unwrap();
    assert_eq!(kinds(&a), ["lag"]);
    assert_eq!((s.missing_due.clone(), s.max_lag_s), (vec![3], 601));
}

#[test]
fn test_deadline_and_unfinalized_and_degraded() {
    let spec = spec();
    let deadline = spec.deadline().unwrap().get();
    let mut filled = vec![true; 25];
    filled[10] = false;
    // 6 h before the deadline with a bucket empty: lag and deadline.
    let (_, a) = assess_window(
        "w",
        deadline - 6 * 3_600,
        &spec,
        WindowState::Sampling,
        &filled,
        T,
    )
    .unwrap();
    assert_eq!(kinds(&a), ["lag", "deadline"]);
    // Past the deadline and not finalized: lag and unfinalized.
    let (_, a) =
        assess_window("w", deadline + 1, &spec, WindowState::Sampling, &filled, T).unwrap();
    assert_eq!(kinds(&a), ["lag", "unfinalized"]);
    // Degraded.
    let (s, a) =
        assess_window("w", deadline + 1, &spec, WindowState::Degraded, &filled, T).unwrap();
    assert_eq!((kinds(&a), s.state), (vec!["degraded"], "degraded"));
    // Complete, more than the grace after the end, still sampling: unfinalized only.
    let end = spec.end().get();
    let (_, a) =
        assess_window("w", end + 601, &spec, WindowState::Sampling, &[true; 25], T).unwrap();
    assert_eq!(kinds(&a), ["unfinalized"]);
    // Finalized: nothing.
    let (_, a) = assess_window(
        "w",
        end + 601,
        &spec,
        WindowState::Finalized,
        &[true; 25],
        T,
    )
    .unwrap();
    assert!(a.is_empty());
}

fn lag() -> Alert {
    Alert {
        kind: "lag",
        scope: "window:w".into(),
        subject: "w".into(),
        message: "m".into(),
    }
}

fn observed(s: &[&str]) -> Vec<String> {
    s.iter().map(|x| (*x).to_string()).collect()
}

#[test]
fn test_alert_book_delivers_once_then_realerts_and_resolves() {
    let a = lag();
    let seen = observed(&["window:w"]);
    let mut book = AlertBook::default();
    let (due, resolved) = book.update(std::slice::from_ref(&a), &seen, 0, 3_600);
    assert_eq!((due.len(), resolved.len()), (1, 0));
    book.delivered(&a, 0);
    let (due, _) = book.update(std::slice::from_ref(&a), &seen, 3_599, 3_600);
    assert!(due.is_empty());
    let (due, _) = book.update(std::slice::from_ref(&a), &seen, 3_600, 3_600);
    assert_eq!(due.len(), 1);
    book.delivered(&a, 3_600);
    let (due, resolved) = book.update(&[], &seen, 3_700, 3_600);
    assert!(due.is_empty());
    assert_eq!(resolved, vec![("lag", "w".to_string())]);
    // Active again after resolving: due at once.
    let (due, _) = book.update(std::slice::from_ref(&a), &seen, 3_701, 3_600);
    assert_eq!(due.len(), 1);
}

#[test]
fn test_unobserved_scope_never_resolves() {
    let a = lag();
    let mut book = AlertBook::default();
    let _ = book.update(std::slice::from_ref(&a), &observed(&["window:w"]), 0, 3_600);
    book.delivered(&a, 0);
    // The window could not be read (or the whole pass failed): not reported, not observed.
    let (_, resolved) = book.update(&[], &observed(&["rpc", "balance:p"]), 60, 3_600);
    assert!(resolved.is_empty(), "resolved without seeing the window");
    let (_, resolved) = book.update(&[], &[], 120, 3_600);
    assert!(resolved.is_empty());
    // Observed again and gone: resolved.
    let (_, resolved) = book.update(&[], &observed(&["window:w"]), 180, 3_600);
    assert_eq!(resolved.len(), 1);
}

#[test]
fn test_failed_delivery_is_retried_at_the_next_pass() {
    let a = lag();
    let seen = observed(&["window:w"]);
    let mut book = AlertBook::default();
    let (due, _) = book.update(std::slice::from_ref(&a), &seen, 0, 3_600);
    assert_eq!(due.len(), 1);
    // The command failed: not marked delivered, so due again 60 s later, not after an hour.
    let (due, _) = book.update(std::slice::from_ref(&a), &seen, 60, 3_600);
    assert_eq!(due.len(), 1);
    book.delivered(&a, 60);
    let (due, _) = book.update(std::slice::from_ref(&a), &seen, 120, 3_600);
    assert!(due.is_empty());
}

#[test]
fn test_vaa_tracker_ages_by_first_sight_and_forgets_closed() {
    let mut t = VaaTracker::default();
    let open = vec!["v1".to_string()];
    assert!(t.observe("p", &open, 100, 50).is_empty());
    assert!(t.observe("p", &open, 150, 50).is_empty());
    assert_eq!(kinds(&t.observe("p", &open, 151, 50)), ["encoded-vaa"]);
    // Closed, then a new one with the same address seen later starts from zero.
    assert!(t.observe("p", &[], 200, 50).is_empty());
    assert!(t.observe("p", &open, 300, 50).is_empty());
}

#[test]
fn test_vaa_tracker_keeps_each_payers_ages_apart() {
    // Primary and standby payers observed in turn every pass: a leftover of each must age.
    let mut t = VaaTracker::default();
    let (a, b) = (vec!["va".to_string()], vec!["vb".to_string()]);
    for h in [100, 120, 140] {
        assert!(t.observe("pa", &a, h, 50).is_empty());
        assert!(t.observe("pb", &b, h, 50).is_empty());
    }
    let fired: Vec<String> = t
        .observe("pa", &a, 151, 50)
        .into_iter()
        .chain(t.observe("pb", &b, 151, 50))
        .map(|x| x.subject)
        .collect();
    assert_eq!(fired, ["va", "vb"]);
}

#[test]
fn test_standby_lag_bounds() {
    assert!(Keeper::check_standby_lag(600, 3_600).is_ok());
    assert!(Keeper::check_standby_lag(3_300, 3_600).is_ok());
    assert!(Keeper::check_standby_lag(599, 3_600).is_err());
    assert!(Keeper::check_standby_lag(3_301, 3_600).is_err());
    // 900 s intervals allow exactly 600.
    assert!(Keeper::check_standby_lag(600, 900).is_ok());
    assert!(Keeper::check_standby_lag(601, 900).is_err());
    // Shorter intervals allow no standby.
    assert!(Keeper::check_standby_lag(600, 600).is_err());
    assert!(Keeper::check_standby_lag(600, 60).is_err());
}
