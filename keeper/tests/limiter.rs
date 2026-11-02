//! The host-wide Hermes limiter over real HTTP, from two processes
//! (`docs/PLAN-improvements.md`, I6): a mock Hermes on a local socket records when each
//! request arrives; two child processes (this test binary run again) backfill through their own
//! `HermesSource`, sharing one limiter file. Scaled clock: a 1,000 ms window with `R = 3`
//! instead of 60 s, so the test takes seconds.
//!
//! Asserted: no sliding window holds more than `R` requests (every request counted, 404s and
//! the 429 included), every planned request is eventually answered, and after the mock answers
//! one 429 no request from either process takes a slot during the block (requests already in
//! flight, at most `R - 1`, may still arrive).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::disallowed_macros,
    missing_docs
)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use varswap_keeper::{
    HermesConfig, HermesLimiter, HermesSource, KeeperError, UreqHttp, HERMES_API_KEY_VAR,
    HERMES_URL_VAR,
};

const R: u32 = 3;
const WINDOW_MS: u64 = 1_000;
const BLOCK_MS: u64 = 1_100;
const PER_CHILD: i64 = 9;
/// The mock answers 429 to this request (1-based, across both children).
const RATE_LIMITED_AT: usize = 7;
/// Slack for the time between a slot being taken and the request reaching the mock: process
/// scheduling and loopback latency on a loaded CI runner (a 50 ms margin flaked on GitHub's).
/// Without the limiter the 19 requests arrive within a few milliseconds, and `R + 1` slots per
/// window would still put 4 arrivals in 750 ms, so the checks keep their power.
const JITTER_MS: u64 = 250;

const CHILD_VAR: &str = "VARSWAP_LIMITER_CHILD";
const URL_VAR: &str = "VARSWAP_LIMITER_URL";
const DIR_VAR: &str = "VARSWAP_LIMITER_DIR";

/// One child: `PER_CHILD` distinct queries (each a miss: one HTTP request), retrying a query
/// that came back rate limited.
#[test]
#[ignore = "run by test_two_processes_share_the_budget as a child process"]
fn child_backfill() {
    let Ok(child) = std::env::var(CHILD_VAR) else {
        return;
    };
    let url = std::env::var(URL_VAR).unwrap();
    let dir = PathBuf::from(std::env::var(DIR_VAR).unwrap());
    let config = HermesConfig::from_lookup(|n| match n {
        HERMES_API_KEY_VAR => Some("test-key".to_string()),
        HERMES_URL_VAR => Some(url.clone()),
        _ => None,
    })
    .unwrap();
    let limiter = HermesLimiter::with_window(&dir, "test-key", R, WINDOW_MS, BLOCK_MS).unwrap();
    let source = HermesSource::new(config, UreqHttp::new()).with_limiter(limiter);
    let base: i64 = if child == "a" { 1_000 } else { 2_000 };
    let mut t = base;
    while t < base + PER_CHILD {
        match source.first_at_or_after(&[7; 32], t) {
            Ok(None) => t += 1,
            Err(KeeperError::RateLimited) => {} // retried after the shared block
            other => panic!("child {child}: unexpected {other:?}"),
        }
    }
}

#[test]
fn test_two_processes_share_the_budget() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let arrivals: Arc<Mutex<Vec<(Instant, u16)>>> = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&arrivals);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut s = stream.unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 1024];
            while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = s.read(&mut chunk).unwrap();
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            let mut l = log.lock().unwrap();
            let status = if l.len() + 1 == RATE_LIMITED_AT {
                429
            } else {
                404
            };
            l.push((Instant::now(), status));
            drop(l);
            let reason = if status == 429 {
                "Too Many Requests"
            } else {
                "Not Found"
            };
            let _ = write!(
                s,
                "HTTP/1.1 {status} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
        }
    });

    let dir = std::env::temp_dir().join(format!("varswap-limiter-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let exe = std::env::current_exe().unwrap();
    let spawn = |name: &str| {
        Command::new(&exe)
            .args(["--exact", "child_backfill", "--ignored", "--nocapture"])
            .env(CHILD_VAR, name)
            .env(URL_VAR, &url)
            .env(DIR_VAR, &dir)
            .spawn()
            .unwrap()
    };
    let started = Instant::now();
    let (mut a, mut b) = (spawn("a"), spawn("b"));
    assert!(a.wait().unwrap().success(), "child a failed");
    assert!(b.wait().unwrap().success(), "child b failed");
    let elapsed = started.elapsed();
    let _ = std::fs::remove_dir_all(&dir);

    let arrivals = arrivals.lock().unwrap().clone();
    // Every query answered once, plus the one retried after the 429.
    let total = usize::try_from(2 * PER_CHILD).unwrap() + 1;
    assert_eq!(arrivals.len(), total, "requests that reached the mock");
    assert_eq!(arrivals.iter().filter(|(_, s)| *s == 429).count(), 1);

    // At most R in any sliding window. Arrival times include scheduling and loopback latency,
    // so the window checked is `JITTER_MS` shorter than the limiter's.
    let window = Duration::from_millis(WINDOW_MS - JITTER_MS);
    for (i, (t0, _)) in arrivals.iter().enumerate() {
        let n = arrivals[i..]
            .iter()
            .take_while(|(t, _)| t.duration_since(*t0) < window)
            .count();
        assert!(
            n <= usize::try_from(R).unwrap(),
            "{n} requests within {window:?} starting at request {i}"
        );
    }

    // After the 429: requests that already held a slot (at most R - 1, sent in the same
    // window, in flight within `JITTER_MS`) may still arrive; nothing else arrives
    // until the block has passed.
    let (t429, _) = arrivals[RATE_LIMITED_AT - 1];
    let in_flight = Duration::from_millis(JITTER_MS);
    let after: Vec<Duration> = arrivals[RATE_LIMITED_AT..]
        .iter()
        .map(|(t, _)| t.duration_since(t429))
        .collect();
    let early = after.iter().filter(|d| **d <= in_flight).count();
    assert!(
        early < usize::try_from(R).unwrap(),
        "{early} requests right after the 429"
    );
    let next = after.iter().find(|d| **d > in_flight).unwrap();
    assert!(
        *next >= Duration::from_millis(BLOCK_MS - JITTER_MS),
        "a request took a slot {next:?} after the 429, inside the block"
    );

    // The budget, not the clients, set the pace: 19 requests at 3 per window need at least
    // 6 full windows after the first (100 ms of slack for process start-up timing).
    assert!(
        elapsed >= Duration::from_millis(6 * WINDOW_MS - 100),
        "{elapsed:?}"
    );
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("varswap-limiter-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// Crash controls: whatever a killed writer can leave behind never frees the budget.
#[test]
fn test_cut_or_empty_state_is_read_conservatively() {
    for (name, text) in [("cut", "0\n"), ("empty", ""), ("partial", "0\n1\n2\n")] {
        let dir = scratch(name);
        let l = HermesLimiter::with_window(&dir, "k", 3, 300, 400).unwrap();
        std::fs::write(l.path(), text).unwrap();
        let t = Instant::now();
        l.acquire().unwrap();
        assert!(
            t.elapsed() >= Duration::from_millis(390),
            "{name}: a broken state let a request through after {:?}",
            t.elapsed()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn test_missing_state_is_a_fresh_budget_and_a_dead_temp_file_changes_nothing() {
    let dir = scratch("tmp");
    let l = HermesLimiter::with_window(&dir, "k", 3, 600, 700).unwrap();
    let t = Instant::now();
    for _ in 0..3 {
        l.acquire().unwrap();
    }
    assert!(
        t.elapsed() < Duration::from_millis(200),
        "fresh budget: {:?}",
        t.elapsed()
    );
    // A writer killed before its rename leaves a temporary file; the state is the old one.
    let mut tmp = l.path().as_os_str().to_owned();
    tmp.push(".tmp");
    std::fs::write(&tmp, "0\nend\n").unwrap();
    l.acquire().unwrap();
    assert!(
        t.elapsed() >= Duration::from_millis(590),
        "the fourth request left after {:?}, inside the window",
        t.elapsed()
    );
    let _ = std::fs::remove_dir_all(&dir);
}
