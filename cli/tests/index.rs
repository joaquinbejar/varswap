//! The realized volatility index (`docs/PLAN-phase2.md`, P1), end to end over a fake cluster:
//! windows built with `varswap-core`, exported through the real record path, read back offline.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::panic_in_result_fn,
    clippy::integer_division,
    missing_docs
)]

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};
use varswap_cli::index::{
    check_stubs, history, index_at, read_sync, resolve_as_of, series_sync, Pace, Paced, Series,
    SeriesCluster, Snapshot, SYNC_FILE,
};
use varswap_cli::record::{self, History};
use varswap_cli::{AccountInfo, CliError};
use varswap_core::{
    accept_print, window_size, Bps, FeedId, LnKernel, RawPrint, ReturnKernel, RiskParams, Seconds,
    SumSq, UnixTimestamp, Window, WindowSpec,
};
use varswap_sdk::Address;

const DAY: i64 = 86_400;
const H: i64 = 3_600;
const FIRST: i64 = 1_793_638_800; // 2026-11-02T17:00:00Z
const PAYER: &str = "5MTVStedYVor97UjAYU4shkp6RaTiKXEFupfHJN79D1";

fn series() -> Series {
    Series::parse(&json!({
        "id": "test",
        "feed": "ef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d",
        "anchor_hour": 17,
        "interval": 3600,
        "first_start": "2026-11-02T17:00:00Z",
        "program_id": "BWqsaXvASrbErApP1ovz7vSvBLYqXGaMaZANEYf1xrB9",
        "cluster": "devnet",
    }))
    .unwrap()
}

fn t(s: i64) -> UnixTimestamp {
    UnixTimestamp::new(s)
}

/// How a test window is filled.
#[derive(Clone)]
struct Fill {
    /// The runs, as (first, last) buckets.
    runs: Vec<(u32, u32)>,
    /// Seconds from a run's first boundary to its transaction's block time.
    delay: i64,
    /// Finalize at this point; `None` leaves it open.
    finalize: Option<i64>,
}

impl Fill {
    /// Every bucket its own run, finalized at `end`.
    fn full() -> Self {
        Self {
            runs: (0..25).map(|k| (k, k)).collect(),
            delay: 30,
            finalize: Some(DAY),
        }
    }

    /// `runs` plus single buckets for the rest, finalized at `end`.
    fn with_runs(runs: &[(u32, u32)]) -> Self {
        let mut all: Vec<(u32, u32)> = runs.to_vec();
        for k in 0..25 {
            if !runs.iter().any(|(a, b)| (*a..=*b).contains(&k)) {
                all.push((k, k));
            }
        }
        all.sort_unstable();
        Self {
            runs: all,
            delay: 30,
            finalize: Some(DAY),
        }
    }

    /// Only the first `filled` buckets, degraded after the deadline.
    fn degraded(filled: u32) -> Self {
        Self {
            runs: (0..filled).map(|k| (k, k)).collect(),
            delay: 30,
            finalize: Some(DAY + 24 * H + 1),
        }
    }

    fn open(filled: u32) -> Self {
        Self {
            finalize: None,
            ..Self::degraded(filled)
        }
    }
}

#[derive(Clone)]
struct Built {
    data: Vec<u8>,
    signatures: Vec<String>,
    transactions: HashMap<String, Value>,
}

fn tx(
    s: &Series,
    window: &Address,
    sig: &str,
    slot: u64,
    time: i64,
    data: &str,
    logs: &[String],
) -> Value {
    json!({
        "slot": slot,
        "blockTime": time,
        "meta": { "err": null, "logMessages": logs },
        "transaction": {
            "signatures": [sig],
            "message": {
                "accountKeys": [PAYER, window.to_string(), s.program_id.to_string()],
                "instructions": [{ "programIdIndex": 2, "accounts": [1], "data": data }],
            }
        }
    })
}

/// A window of day `day`, from the real core code, and the transactions that made it.
fn build(s: &Series, day: u32, fill: &Fill) -> Built {
    let address = s.address(day).unwrap();
    let (start, end) = (s.start(day).unwrap(), s.end(day).unwrap());
    let spec = WindowSpec::try_new(
        FeedId::new(s.feed),
        t(start),
        t(end),
        Seconds::new(3_600),
        RiskParams {
            tolerance: Seconds::new(6 * 3_600),
            max_conf: Bps::new(200),
            backfill_deadline: Seconds::new(24 * 3_600),
            receiver_program: [7; 32],
            exponent: -8,
        },
    )
    .unwrap();
    let mut data = vec![0u8; window_size(spec.buckets().unwrap()).unwrap()];
    let mut w = Window::init(&mut data, spec, 255).unwrap();
    let mut transactions = HashMap::new();
    let mut order = Vec::new();
    let mut push = |sig: String, v: Value, order: &mut Vec<String>| {
        transactions.insert(sig.clone(), v);
        order.push(sig);
    };
    let program = s.program_id;
    let invoke = format!("Program {program} invoke [1]");
    let done = format!("Program {program} success");
    let sig = format!("create{day}");
    push(
        sig.clone(),
        tx(
            s,
            &address,
            &sig,
            1,
            start - H,
            "3",
            &[invoke.clone(), done.clone()],
        ),
        &mut order,
    );
    for (i, (a, b)) in fill.runs.iter().enumerate() {
        let boundary = |k: u32| start + i64::from(k) * H;
        // A different price each day and bucket: every window has its own variance.
        let price = 15_000_000_000 + i64::from(day) * 7_000_000 + i64::from(*a % 5) * 13_000_000;
        let raw = RawPrint {
            feed: FeedId::new(s.feed),
            price,
            conf: 1,
            exponent: -8,
            publish_time: t(boundary(*b)),
            prev_publish_time: t(boundary(*a) - 1),
        };
        let accepted = accept_print(&spec, &raw).unwrap();
        let time = boundary(*a) + fill.delay;
        let run = w
            .sample_accepted(&accepted, t(time.max(boundary(*b))), &LnKernel)
            .unwrap();
        assert_eq!((run.first(), run.last()), (*a, *b));
        let sig = format!("sample{day}x{a}");
        let event = format!(
            "Program log: varswap:sample first={a} last={b} publish_time={} price={price} expo=-8",
            boundary(*b)
        );
        let v = tx(
            s,
            &address,
            &sig,
            2 + i as u64,
            time,
            "4",
            &[invoke.clone(), event, done.clone()],
        );
        push(sig, v, &mut order);
    }
    if let Some(at) = fill.finalize {
        w.finalize(t(start + at), &LnKernel).unwrap();
        let sig = format!("finalize{day}");
        let v = tx(s, &address, &sig, 100, start + at, "5", &[invoke, done]);
        push(sig, v, &mut order);
    }
    order.reverse(); // the cluster lists newest first
    Built {
        data,
        signatures: order,
        transactions,
    }
}

#[derive(Default)]
struct Fake {
    windows: HashMap<String, Built>,
    owner: Option<Address>,
    clock: i64,
    /// The clock of the snapshot of one address, older than the rest.
    behind: HashMap<String, i64>,
    /// `snapshot` answers without a clock this many times.
    no_clock: u32,
    /// Every call fails as transient this many times.
    transient: u32,
    /// Every call fails as final this many times.
    fail: u32,
    /// Transactions the cluster lists and cannot return.
    unavailable: Vec<String>,
    calls: u32,
    exports: u32,
    /// What `account_at` answers instead of the truth, one per call: `None` is the truth.
    stale_reads: VecDeque<Option<(u64, Vec<u8>)>>,
    /// A file another writer creates during the next `account_at`.
    race: Option<(PathBuf, String)>,
}

impl Fake {
    fn new(clock: i64) -> Self {
        Self {
            owner: Some(series().program_id),
            clock,
            ..Self::default()
        }
    }

    fn put(mut self, day: u32, fill: &Fill) -> Self {
        let s = series();
        self.windows
            .insert(s.address(day).unwrap().to_string(), build(&s, day, fill));
        self
    }

    fn gate(&mut self) -> Result<(), CliError> {
        self.calls += 1;
        if self.fail > 0 {
            self.fail -= 1;
            return Err(CliError::Rpc("final".to_owned()));
        }
        if self.transient > 0 {
            self.transient -= 1;
            return Err(CliError::RpcTransient("429".to_owned()));
        }
        Ok(())
    }

    fn account(&self, address: &Address) -> Option<AccountInfo> {
        self.windows.get(&address.to_string()).map(|b| AccountInfo {
            owner: self.owner.unwrap(),
            data: b.data.clone(),
        })
    }
}

impl History for Fake {
    fn signatures(&mut self, a: &Address, before: Option<&str>) -> Result<Vec<String>, CliError> {
        self.gate()?;
        if before.is_some() {
            return Ok(vec![]);
        }
        Ok(self
            .windows
            .get(&a.to_string())
            .map_or_else(Vec::new, |b| b.signatures.clone()))
    }

    fn transaction(&mut self, sig: &str) -> Result<Option<Value>, CliError> {
        self.gate()?;
        self.exports += 1;
        if self.unavailable.iter().any(|u| u == sig) {
            return Ok(None);
        }
        Ok(self
            .windows
            .values()
            .find_map(|b| b.transactions.get(sig).cloned()))
    }

    fn account_at(&mut self, a: &Address) -> Result<(u64, Option<AccountInfo>), CliError> {
        self.gate()?;
        if let Some((path, text)) = self.race.take() {
            std::fs::write(path, text).unwrap();
        }
        let current = u64::try_from(self.clock).unwrap();
        if let Some(Some((slot, data))) = self.stale_reads.pop_front() {
            let owner = self.owner.unwrap();
            return Ok((slot, Some(AccountInfo { owner, data })));
        }
        Ok((current, self.account(a)))
    }
}

impl SeriesCluster for Fake {
    fn snapshot(&mut self, a: &Address) -> Result<Snapshot, CliError> {
        self.gate()?;
        if self.no_clock > 0 {
            self.no_clock -= 1;
            return Err(CliError::Rpc("getMultipleAccounts: no clock".to_owned()));
        }
        let clock = self
            .behind
            .get(&a.to_string())
            .copied()
            .unwrap_or(self.clock);
        Ok(Snapshot {
            slot: u64::try_from(clock).unwrap(),
            clock,
            account: self.account(a),
        })
    }
}

fn scratch(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("index")
        .join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Clock just after `n` days have ended.
const fn after(n: i64) -> i64 {
    FIRST + n * DAY + 60
}

fn value(out: &Value, days: u64) -> &Value {
    out["values"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["days"] == days)
        .unwrap()
}

fn files(dir: &std::path::Path) -> Vec<String> {
    let mut f: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    f.sort();
    f
}

fn file_of(s: &Series, dir: &std::path::Path, day: u32) -> PathBuf {
    dir.join(s.file_name(day).unwrap())
}

/// Syncs a fake holding `fills` (by day) with the clock past all of them.
fn synced(name: &str, fills: &[(u32, Fill)], days_ended: i64) -> (Series, PathBuf) {
    let s = series();
    let dir = scratch(name);
    let mut f = Fake::new(after(days_ended));
    for (day, fill) in fills {
        f = f.put(*day, fill);
    }
    series_sync(&mut f, &s, &dir).unwrap();
    (s, dir)
}

fn sum_sq_of(s: &Series, dir: &std::path::Path, day: u32) -> u128 {
    let v: Value =
        serde_json::from_str(&std::fs::read_to_string(file_of(s, dir, day)).unwrap()).unwrap();
    v["derived"]["sum_sq_raw"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}

// --- values ---

#[test]
fn test_rv_1_equals_the_stored_realized_variance() {
    let (s, dir) = synced("rv1", &[(0, Fill::full())], 1);
    let v: Value =
        serde_json::from_str(&std::fs::read_to_string(file_of(&s, &dir, 0)).unwrap()).unwrap();
    let out = index_at(&s, &dir, after(1), None).unwrap();
    let one = value(&out, 1);
    assert_eq!(one["label"], "complete");
    assert_eq!(one["rv_raw"], v["derived"]["realized_var_raw"]);
    assert_eq!(one["rv"], v["derived"]["realized_var"]);
}

#[test]
fn test_rv_k_is_the_sum_over_the_span_of_usable_days() {
    let fills: Vec<(u32, Fill)> = (0..7).map(|d| (d, Fill::full())).collect();
    let (s, dir) = synced("rv7", &fills, 7);
    let sum: u128 = (0..7).map(|d| sum_sq_of(&s, &dir, d)).sum();
    let want = LnKernel
        .annualize(SumSq::from_raw(sum), 7 * 86_400)
        .unwrap();
    let out = index_at(&s, &dir, after(7), None).unwrap();
    assert_eq!(value(&out, 7)["rv_raw"], want.to_string());
    assert_eq!(value(&out, 7)["label"], "complete");
    // A value over some days is not the mean of the 1 day values' annualization of one day.
    assert_ne!(value(&out, 1)["rv_raw"], want.to_string());
}

#[test]
fn test_an_excluded_day_shrinks_the_span_it_is_never_zero_or_interpolated() {
    let mut fills: Vec<(u32, Fill)> = (0..7).map(|d| (d, Fill::full())).collect();
    fills[3] = (3, Fill::degraded(10));
    let (s, dir) = synced("span", &fills, 7);
    let sum: u128 = [0, 1, 2, 4, 5, 6]
        .iter()
        .map(|d| sum_sq_of(&s, &dir, *d))
        .sum();
    let want = LnKernel
        .annualize(SumSq::from_raw(sum), 6 * 86_400)
        .unwrap();
    let out = index_at(&s, &dir, after(7), None).unwrap();
    assert_eq!(value(&out, 7)["rv_raw"], want.to_string());
    assert_eq!(value(&out, 7)["usable"], 6);
    assert_eq!(value(&out, 7)["label"], "partial");
}

#[test]
fn test_committed_records_annualize_to_their_stored_variance() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../docs/windows");
    let mut seen = 0;
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_none_or(|x| x != "json") {
            continue;
        }
        let b: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        let d = record::verify_offline(&b).unwrap();
        let data = record::account_bytes(&b).unwrap();
        let info = Window::inspect(&data).unwrap();
        let span = info.spec.span_seconds().unwrap();
        let sum: u128 = d["sum_sq_raw"].as_str().unwrap().parse().unwrap();
        let got = LnKernel.annualize(SumSq::from_raw(sum), span).unwrap();
        assert_eq!(
            d["realized_var_raw"].as_str().unwrap(),
            got.to_string(),
            "{p:?}"
        );
        seen += 1;
    }
    assert!(seen >= 2);
}

// --- labels ---

#[test]
fn test_label_compressed_and_missed_samples_of_runs_1_3_5() {
    let (s, dir) = synced("runs", &[(0, Fill::with_runs(&[(2, 4), (10, 14)]))], 1);
    let out = index_at(&s, &dir, after(1), None).unwrap();
    let one = value(&out, 1);
    assert_eq!(one["label"], "compressed");
    assert_eq!(one["compressed_buckets"], 2 + 4);
    assert_eq!(one["compressed_runs"], 2);
    assert_eq!(one["longest_run"], 5);
    assert_eq!(one["missed_samples"], 6);
    assert_eq!(one["empty_buckets"], 0);
}

#[test]
fn test_missed_samples_adds_a_degraded_days_empty_buckets() {
    let (s, dir) = synced(
        "missed",
        &[(0, Fill::with_runs(&[(2, 4)])), (1, Fill::degraded(15))],
        2,
    );
    let out = index_at(&s, &dir, after(2), None).unwrap();
    let seven = value(&out, 7);
    assert_eq!(seven["compressed_buckets"], 2);
    assert_eq!(seven["empty_buckets"], 10);
    assert_eq!(seven["missed_samples"], 12);
}

#[test]
fn test_threshold_edges_for_seven_days() {
    let mut fills: Vec<(u32, Fill)> = (0..7).map(|d| (d, Fill::full())).collect();
    for f in fills.iter_mut().take(3) {
        f.1 = Fill::degraded(1);
    }
    let (s, dir) = synced("t4", &fills, 7);
    let out = index_at(&s, &dir, after(7), None).unwrap();
    assert_eq!(value(&out, 7)["usable"], 4);
    assert_eq!(value(&out, 7)["label"], "partial");
    assert!(value(&out, 7)["sigma"].is_string());

    fills[3].1 = Fill::degraded(1);
    let (s, dir) = synced("t3", &fills, 7);
    let out = index_at(&s, &dir, after(7), None).unwrap();
    assert_eq!(value(&out, 7)["usable"], 3);
    assert_eq!(value(&out, 7)["label"], "insufficient");
    assert!(value(&out, 7)["sigma"].is_null());
    assert_eq!(value(&out, 7)["excluded"].as_array().unwrap().len(), 4);
}

#[test]
fn test_each_exclusion_reason() {
    let s = series();
    let dir = scratch("reasons");
    let mut f = Fake::new(after(5))
        .put(0, &Fill::full())
        .put(1, &Fill::degraded(3))
        // Day 2 is never created: `missing`.
        .put(3, &Fill::open(3))
        .put(4, &Fill::full());
    series_sync(&mut f, &s, &dir).unwrap();
    // `unknown` has its own test: it takes a run past the retry period.
    let out = index_at(&s, &dir, after(5), None).unwrap();
    let seven = value(&out, 7);
    let reasons: HashMap<u64, String> = seven["excluded"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["day"].as_u64().unwrap(),
                e["reason"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(reasons[&1], "degraded");
    assert_eq!(reasons[&2], "missing");
    assert_eq!(reasons[&3], "open");
    assert_eq!(reasons.len(), 3);
    assert_eq!(seven["before_series"], 2);
    assert_eq!(seven["usable"], 2);
    assert_eq!(seven["label"], "insufficient");
}

#[test]
fn test_days_before_the_series_make_a_value_partial_and_say_so() {
    let (s, dir) = synced(
        "before",
        &[
            (0, Fill::full()),
            (1, Fill::full()),
            (2, Fill::full()),
            (3, Fill::full()),
        ],
        4,
    );
    let out = index_at(&s, &dir, after(4), None).unwrap();
    assert_eq!(value(&out, 7)["before_series"], 3);
    assert_eq!(value(&out, 7)["label"], "partial");
    assert_eq!(value(&out, 30)["before_series"], 26);
    assert_eq!(value(&out, 30)["label"], "insufficient");
    assert_eq!(value(&out, 1)["before_series"], 0);
}

#[test]
fn test_late_samples_either_side_of_the_grace() {
    for (delay, want) in [(600, 0), (601, 25)] {
        let fill = Fill {
            delay,
            ..Fill::full()
        };
        let (s, dir) = synced(&format!("late{delay}"), &[(0, fill)], 1);
        let out = index_at(&s, &dir, after(1), None).unwrap();
        assert_eq!(value(&out, 1)["late_samples"], want, "delay {delay}");
        // Lateness is the keeper's record: it changes no price and no run.
        assert_eq!(value(&out, 1)["label"], "complete");
    }
}

// --- the default day ---

#[test]
fn test_default_day_is_the_last_ended_not_the_last_with_a_record() {
    let s = series();
    let dir = scratch("default-open");
    let mut f = Fake::new(after(2))
        .put(0, &Fill::full())
        .put(1, &Fill::open(5));
    series_sync(&mut f, &s, &dir).unwrap();
    assert!(file_of(&s, &dir, 0).exists());
    assert!(!file_of(&s, &dir, 1).exists());
    let as_of = resolve_as_of(&dir, None).unwrap();
    assert_eq!(as_of, after(2));
    let out = index_at(&s, &dir, as_of, None).unwrap();
    assert_eq!(out["day"], 1);
    let one = value(&out, 1);
    assert_eq!(one["label"], "insufficient");
    assert!(one["sigma"].is_null());
    assert_eq!(one["excluded"][0]["reason"], "open");
    let h = history(&s, &dir, as_of).unwrap();
    assert_eq!(h["last_day"], 1);
    assert_eq!(h["history"].as_array().unwrap().len(), 2);
    assert_eq!(value(&h["history"][1], 1)["excluded"][0]["reason"], "open");
    assert_eq!(value(&h["history"][0], 1)["label"], "complete");
}

#[test]
fn test_a_last_day_missing_is_reported_as_missing() {
    let (s, dir) = synced("default-missing", &[(0, Fill::full())], 2);
    let out = index_at(&s, &dir, after(2), None).unwrap();
    assert_eq!(out["day"], 1);
    assert_eq!(value(&out, 1)["excluded"][0]["reason"], "missing");
    assert_eq!(value(&out, 1)["label"], "insufficient");
}

#[test]
fn test_no_records_reports_every_ended_day_open() {
    let s = series();
    let dir = scratch("no-records");
    let out = index_at(&s, &dir, after(3), None).unwrap();
    assert_eq!(out["day"], 2);
    assert_eq!(value(&out, 7)["usable"], 0);
    assert_eq!(value(&out, 7)["excluded"].as_array().unwrap().len(), 3);
    assert!(value(&out, 7)["excluded"]
        .as_array()
        .unwrap()
        .iter()
        .all(|e| e["reason"] == "open"));
    assert!(value(&out, 7)["sigma"].is_null());
}

#[test]
fn test_no_as_of_fails_and_a_day_past_as_of_fails() {
    let s = series();
    let dir = scratch("as-of");
    let e = resolve_as_of(&dir, None).unwrap_err().to_string();
    assert!(e.contains("--as-of"), "{e}");
    assert_eq!(resolve_as_of(&dir, Some(5)).unwrap(), 5);
    let e = index_at(&s, &dir, after(3), Some(3))
        .unwrap_err()
        .to_string();
    assert!(e.contains("has not ended"), "{e}");
    let e = index_at(&s, &dir, FIRST, Some(0)).unwrap_err().to_string();
    assert!(e.contains("has not ended"), "{e}");
}

#[test]
fn test_before_any_day_ends_there_is_no_value_and_no_error() {
    let s = series();
    let dir = scratch("before-any");
    let out = index_at(&s, &dir, FIRST + 10, None).unwrap();
    assert!(out["last_day"].is_null());
    let h = history(&s, &dir, FIRST + 10).unwrap();
    assert!(h["history"].as_array().unwrap().is_empty());
}

// --- snapshot context ---

#[test]
fn test_null_from_a_snapshot_before_end_writes_no_stub_and_a_later_one_does() {
    let s = series();
    let dir = scratch("old-snapshot");
    let address = s.address(0).unwrap().to_string();
    let mut f = Fake::new(after(1));
    // The first read fixes as_of past day 0; the node then answers day 0 from before its end.
    let mut stale = Fake::new(after(1));
    stale.behind.insert(address, FIRST + 100);
    // as_of comes from day 0's own snapshot here, so seed it from a previous run instead.
    std::fs::write(
        dir.join(SYNC_FILE),
        json!({ "as_of": after(1), "slot": 1 }).to_string(),
    )
    .unwrap();
    series_sync(&mut stale, &s, &dir).unwrap();
    assert!(!file_of(&s, &dir, 0).exists(), "a stale null wrote a stub");
    let out = index_at(&s, &dir, read_sync(&dir).unwrap().unwrap().as_of, None).unwrap();
    assert_eq!(value(&out, 1)["excluded"][0]["reason"], "open");
    let synced: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join(SYNC_FILE)).unwrap()).unwrap();
    assert_eq!(synced["open"][0]["reason"], "snapshot_before_end");

    series_sync(&mut f, &s, &dir).unwrap();
    assert!(file_of(&s, &dir, 0).exists());
    let stub: Value =
        serde_json::from_str(&std::fs::read_to_string(file_of(&s, &dir, 0)).unwrap()).unwrap();
    assert_eq!(stub["kind"], "missing");
    assert_eq!(stub["clock"], after(1));
}

#[test]
fn test_rpc_errors_and_a_missing_clock_write_no_stub() {
    let s = series();
    // A final error on the day's read, after the first snapshot answers.
    let dir = scratch("no-stub-error");
    let mut f = Fake::new(after(1));
    f.fail = 1;
    assert!(series_sync(&mut f, &s, &dir).is_err());
    assert_eq!(files(&dir), Vec::<String>::new());

    // A final failure on the second read: the day's own.
    let dir = scratch("no-stub-clock");
    let mut f = Fake::new(after(1));
    std::fs::write(
        dir.join(SYNC_FILE),
        json!({ "as_of": after(1), "slot": 1 }).to_string(),
    )
    .unwrap();
    f.no_clock = 2;
    assert!(series_sync(&mut f, &s, &dir).is_err());
    assert!(!file_of(&s, &dir, 0).exists());

    struct SecondFails(Fake);
    impl History for SecondFails {
        fn signatures(&mut self, a: &Address, b: Option<&str>) -> Result<Vec<String>, CliError> {
            self.0.signatures(a, b)
        }
        fn transaction(&mut self, s: &str) -> Result<Option<Value>, CliError> {
            self.0.transaction(s)
        }
        fn account_at(&mut self, a: &Address) -> Result<(u64, Option<AccountInfo>), CliError> {
            self.0.account_at(a)
        }
    }
    impl SeriesCluster for SecondFails {
        fn snapshot(&mut self, a: &Address) -> Result<Snapshot, CliError> {
            let r = self.0.snapshot(a);
            if self.0.calls >= 2 {
                return Err(CliError::Rpc("getMultipleAccounts: no clock".to_owned()));
            }
            r
        }
    }
    let dir = scratch("no-stub-second");
    let mut f = SecondFails(Fake::new(after(1)));
    let e = series_sync(&mut f, &s, &dir).unwrap_err().to_string();
    assert!(e.contains("no clock"), "{e}");
    assert!(!file_of(&s, &dir, 0).exists());
    // The run still records how far the chain was, and the day stays open.
    assert_eq!(read_sync(&dir).unwrap().unwrap().as_of, after(1));
    let out = index_at(&s, &dir, after(1), None).unwrap();
    assert_eq!(value(&out, 1)["excluded"][0]["reason"], "open");
}

#[test]
fn test_unknown_is_retried_for_three_days_then_written() {
    let s = series();
    let dir = scratch("unknown");
    let built = build(&s, 0, &Fill::full());
    let lost = built.signatures[3].clone();
    let mut f = Fake::new(after(1)).put(0, &Fill::full());
    f.unavailable.push(lost);
    series_sync(&mut f, &s, &dir).unwrap();
    assert!(
        !file_of(&s, &dir, 0).exists(),
        "written during the retry period"
    );
    let synced_file: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join(SYNC_FILE)).unwrap()).unwrap();
    assert_eq!(synced_file["open"][0]["reason"], "history_unproven");

    // Just before the 3 days: still nothing.
    f.clock = FIRST + DAY + 3 * DAY - 1;
    series_sync(&mut f, &s, &dir).unwrap();
    assert!(!file_of(&s, &dir, 0).exists());

    // Past them, it is written honestly and excluded as `unknown`.
    f.clock = FIRST + DAY + 3 * DAY;
    series_sync(&mut f, &s, &dir).unwrap();
    let out = index_at(&s, &dir, f.clock, Some(0)).unwrap();
    assert_eq!(value(&out, 1)["excluded"][0]["reason"], "unknown");
    assert_eq!(value(&out, 1)["label"], "insufficient");
}

#[test]
fn test_history_returning_within_the_retry_period_gives_the_real_label() {
    let s = series();
    let dir = scratch("unknown-recovers");
    let built = build(&s, 0, &Fill::full());
    let mut f = Fake::new(after(1)).put(0, &Fill::full());
    f.unavailable.push(built.signatures[3].clone());
    series_sync(&mut f, &s, &dir).unwrap();
    assert!(!file_of(&s, &dir, 0).exists());
    f.unavailable.clear();
    series_sync(&mut f, &s, &dir).unwrap();
    let out = index_at(&s, &dir, after(1), None).unwrap();
    assert_eq!(value(&out, 1)["label"], "complete");
}

#[test]
fn test_a_lagging_run_does_not_move_as_of_back() {
    let s = series();
    let dir = scratch("as-of-monotone");
    let mut f = Fake::new(after(3))
        .put(0, &Fill::full())
        .put(1, &Fill::full())
        .put(2, &Fill::full());
    series_sync(&mut f, &s, &dir).unwrap();
    assert_eq!(read_sync(&dir).unwrap().unwrap().as_of, after(3));
    f.clock = after(1);
    series_sync(&mut f, &s, &dir).unwrap();
    let state = read_sync(&dir).unwrap().unwrap();
    assert_eq!(state.as_of, after(3));
    assert_eq!(state.slot, u64::try_from(after(3)).unwrap());
    f.clock = after(4);
    series_sync(&mut f, &s, &dir).unwrap();
    assert_eq!(read_sync(&dir).unwrap().unwrap().as_of, after(4));
}

// --- series-sync ---

#[test]
fn test_sync_records_at_once_retries_open_and_is_idempotent() {
    let s = series();
    let dir = scratch("sync");
    // Day 0 full, finalized at `end`: recorded at once, with the deadline not passed.
    let mut f = Fake::new(after(1)).put(0, &Fill::full());
    let out = series_sync(&mut f, &s, &dir).unwrap();
    assert_eq!(out["written"][0]["label"], "complete");
    assert!(file_of(&s, &dir, 0).exists());
    let bytes = std::fs::read(file_of(&s, &dir, 0)).unwrap();

    // A rerun reads the clock and fetches no transaction at all.
    let (calls, exports) = (f.calls, f.exports);
    series_sync(&mut f, &s, &dir).unwrap();
    assert_eq!(f.calls - calls, 1, "a rerun should be one clock read");
    assert_eq!(f.exports, exports);
    assert_eq!(std::fs::read(file_of(&s, &dir, 0)).unwrap(), bytes);

    // Day 1 open: no file, retried; then degraded after its deadline: written.
    let mut f = Fake::new(after(2))
        .put(0, &Fill::full())
        .put(1, &Fill::open(5));
    series_sync(&mut f, &s, &dir).unwrap();
    assert!(!file_of(&s, &dir, 1).exists());
    let mut f = Fake::new(after(3))
        .put(0, &Fill::full())
        .put(1, &Fill::degraded(5))
        .put(2, &Fill::full());
    series_sync(&mut f, &s, &dir).unwrap();
    let out = index_at(&s, &dir, after(3), Some(1)).unwrap();
    assert_eq!(value(&out, 1)["excluded"][0]["reason"], "degraded");
    assert_eq!(value(&out, 1)["empty_buckets"], 20);
}

#[test]
fn test_retries_run_out_with_nothing_written_and_the_next_run_succeeds() {
    let s = series();
    let dir = scratch("retries");
    let mut slept = Vec::new();
    let pace = Pace {
        spacing: Duration::from_secs(1),
        retries: 2,
        backoff: Duration::from_secs(5),
    };
    let mut f = Fake::new(after(1)).put(0, &Fill::full());
    f.transient = 3;
    {
        let mut paced = Paced::new(&mut f, pace, |d| slept.push(d));
        let e = series_sync(&mut paced, &s, &dir).unwrap_err();
        assert!(matches!(e, CliError::RpcTransient(_)), "{e}");
    }
    assert_eq!(slept, [Duration::from_secs(5), Duration::from_secs(10)]);
    assert!(!file_of(&s, &dir, 0).exists());

    let mut slept = Vec::new();
    {
        let mut paced = Paced::new(&mut f, pace, |d| slept.push(d));
        series_sync(&mut paced, &s, &dir).unwrap();
    }
    assert!(file_of(&s, &dir, 0).exists());
    assert!(
        slept.iter().all(|d| *d == Duration::from_secs(1)),
        "paced with no backoff"
    );
    assert!(slept.len() > 50, "only {} calls paced", slept.len());
}

#[test]
fn test_retries_are_for_transient_errors_only() {
    let mut f = Fake::new(after(1));
    f.fail = 1;
    let mut slept = 0;
    let s = series();
    {
        let mut paced = Paced::new(
            &mut f,
            Pace {
                spacing: Duration::ZERO,
                retries: 5,
                backoff: Duration::from_secs(5),
            },
            |_| slept += 1,
        );
        assert!(paced.snapshot(&s.address(0).unwrap()).is_err());
    }
    assert_eq!(f.calls, 1);
    assert_eq!(slept, 0);
}

impl History for &mut Fake {
    fn signatures(&mut self, a: &Address, b: Option<&str>) -> Result<Vec<String>, CliError> {
        (**self).signatures(a, b)
    }
    fn transaction(&mut self, s: &str) -> Result<Option<Value>, CliError> {
        (**self).transaction(s)
    }
    fn account_at(&mut self, a: &Address) -> Result<(u64, Option<AccountInfo>), CliError> {
        (**self).account_at(a)
    }
}

impl SeriesCluster for &mut Fake {
    fn snapshot(&mut self, a: &Address) -> Result<Snapshot, CliError> {
        (**self).snapshot(a)
    }
}

// --- negative controls ---

#[test]
fn test_a_record_failing_offline_verify_is_rejected_by_name() {
    let (s, dir) = synced("forged", &[(0, Fill::full())], 1);
    let path = file_of(&s, &dir, 0);
    let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    v["derived"]["label"] = json!("compressed");
    std::fs::write(&path, v.to_string()).unwrap();
    let e = index_at(&s, &dir, after(1), None).unwrap_err().to_string();
    assert!(e.contains(&s.file_name(0).unwrap()), "{e}");
    assert!(e.contains("derived.label"), "{e}");
}

#[test]
fn test_a_record_of_another_window_is_rejected() {
    let (s, dir) = synced("swapped", &[(0, Fill::full()), (1, Fill::full())], 2);
    // Day 1's record under day 0's name.
    std::fs::copy(file_of(&s, &dir, 1), file_of(&s, &dir, 0)).unwrap();
    let e = index_at(&s, &dir, after(2), None).unwrap_err().to_string();
    assert!(e.contains("another window"), "{e}");
}

#[test]
fn test_a_record_whose_bytes_are_another_days_is_rejected() {
    let (s, dir) = synced("swapped-bytes", &[(0, Fill::full()), (1, Fill::full())], 2);
    let day1: Value =
        serde_json::from_str(&std::fs::read_to_string(file_of(&s, &dir, 1)).unwrap()).unwrap();
    let mut forged = day1;
    forged["window"] = json!(s.address(0).unwrap().to_string());
    std::fs::write(file_of(&s, &dir, 0), forged.to_string()).unwrap();
    let e = index_at(&s, &dir, after(2), None).unwrap_err().to_string();
    assert!(e.contains("spec is not this day's"), "{e}");
}

/// The bytes of day `day`'s window while it was still open.
fn open_bytes(s: &Series, day: u32) -> Vec<u8> {
    build(s, day, &Fill::open(5)).data
}

#[test]
fn test_a_final_snapshot_then_an_export_read_of_a_non_final_window_writes_no_file() {
    let s = series();
    let dir = scratch("stale-state");
    let slot = u64::try_from(after(1)).unwrap();
    let mut f = Fake::new(after(1)).put(0, &Fill::full());
    f.stale_reads.push_back(Some((slot, open_bytes(&s, 0))));
    let out = series_sync(&mut f, &s, &dir).unwrap();
    assert!(!file_of(&s, &dir, 0).exists());
    assert_eq!(out["sync"]["open"][0]["reason"], "stale_read");
    // The node catches up: the next run writes the record.
    let out = series_sync(&mut f, &s, &dir).unwrap();
    assert_eq!(out["written"][0]["label"], "complete");
    assert_eq!(
        value(&index_at(&s, &dir, after(1), None).unwrap(), 1)["label"],
        "complete"
    );
}

#[test]
fn test_an_export_read_from_behind_the_snapshot_writes_no_file() {
    let s = series();
    let dir = scratch("stale-slot");
    let mut f = Fake::new(after(1)).put(0, &Fill::full());
    let data = f.account(&s.address(0).unwrap()).unwrap().data;
    let behind = u64::try_from(after(1)).unwrap() - 1;
    f.stale_reads.push_back(Some((behind, data)));
    let out = series_sync(&mut f, &s, &dir).unwrap();
    assert!(!file_of(&s, &dir, 0).exists());
    assert_eq!(out["sync"]["open"][0]["reason"], "stale_read");
}

#[test]
fn test_a_stale_read_with_its_history_missing_is_not_taken_for_unknown() {
    let s = series();
    let dir = scratch("stale-unknown");
    // Past the retry period, so an `unknown` record would be written if it got that far.
    let later = after(1) + 4 * DAY;
    let slot = u64::try_from(later).unwrap();
    let mut f = Fake::new(later).put(0, &Fill::full());
    let signature = f.windows[&s.address(0).unwrap().to_string()].signatures[0].clone();
    f.unavailable.push(signature);
    f.stale_reads.push_back(Some((slot, open_bytes(&s, 0))));
    let out = series_sync(&mut f, &s, &dir).unwrap();
    assert!(!file_of(&s, &dir, 0).exists());
    assert_eq!(out["sync"]["open"][0]["reason"], "stale_read");
}

#[test]
fn test_a_stale_read_at_the_verify_step_stops_the_run_with_no_file() {
    let s = series();
    let dir = scratch("stale-verify");
    let slot = u64::try_from(after(1)).unwrap();
    let mut f = Fake::new(after(1)).put(0, &Fill::full());
    f.stale_reads.push_back(None);
    f.stale_reads.push_back(Some((slot, open_bytes(&s, 0))));
    let e = series_sync(&mut f, &s, &dir).unwrap_err().to_string();
    assert!(e.contains("differs from the bundle"), "{e}");
    assert!(!file_of(&s, &dir, 0).exists());
}

#[test]
fn test_an_account_at_this_address_with_another_days_spec_is_never_written() {
    let s = series();
    let dir = scratch("wrong-spec");
    let (day0, day1) = (
        s.address(0).unwrap().to_string(),
        s.address(1).unwrap().to_string(),
    );
    // A node answering with day 1's window, consistent with itself, under day 0's address.
    let mut built = build(&s, 1, &Fill::full());
    for tx in built.transactions.values_mut() {
        *tx = serde_json::from_str(&tx.to_string().replace(&day1, &day0)).unwrap();
    }
    let mut f = Fake::new(after(1));
    f.windows.insert(day0, built);
    let e = series_sync(&mut f, &s, &dir).unwrap_err().to_string();
    assert!(e.contains("spec is not this day's"), "{e}");
    assert!(!file_of(&s, &dir, 0).exists());
}

#[test]
fn test_a_record_of_a_non_final_window_is_rejected_whatever_its_label() {
    let s = series();
    let dir = scratch("active-unknown");
    std::fs::create_dir_all(&dir).unwrap();
    let address = s.address(0).unwrap();
    let mut f = Fake::new(after(1)).put(0, &Fill::open(5));
    let signature = f.windows[&address.to_string()].signatures[0].clone();
    f.unavailable.push(signature);
    let bundle = record::export(&mut f, &s.program_id, &address, &s.cluster).unwrap();
    assert_eq!(bundle["derived"]["label"], "unknown");
    std::fs::write(file_of(&s, &dir, 0), bundle.to_string()).unwrap();
    let e = index_at(&s, &dir, after(1), None).unwrap_err().to_string();
    assert!(e.contains("not final"), "{e}");
}

#[test]
fn test_a_sample_with_no_block_time_is_counted_as_unknown_not_on_time() {
    let s = series();
    let dir = scratch("no-block-time");
    let fill = Fill {
        delay: 601,
        ..Fill::full()
    };
    let mut f = Fake::new(after(1)).put(0, &fill);
    let built = f
        .windows
        .get_mut(&s.address(0).unwrap().to_string())
        .unwrap();
    // The bucket 0 sample's transaction; the create and finalize transactions have no samples.
    let first_sample = built
        .transactions
        .values_mut()
        .filter(|tx| tx["blockTime"].as_i64() == Some(FIRST + 601))
        .collect::<Vec<_>>();
    assert_eq!(first_sample.len(), 1);
    for tx in first_sample {
        tx["blockTime"] = Value::Null;
    }
    series_sync(&mut f, &s, &dir).unwrap();
    let out = index_at(&s, &dir, after(1), None).unwrap();
    assert_eq!(value(&out, 1)["late_samples"], 24);
    assert_eq!(value(&out, 1)["late_unknown"], 1);
    // It changes no price and no run.
    assert_eq!(value(&out, 1)["label"], "complete");
}

fn stub_file(name: &str) -> (Series, PathBuf, PathBuf, Value) {
    let (s, dir) = synced(name, &[], 1);
    let path = file_of(&s, &dir, 0);
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["kind"], "missing");
    (s, dir, path, v)
}

#[test]
fn test_a_stub_with_any_field_altered_is_rejected_offline_and_by_check() {
    let (s, dir, path, good) = stub_file("stub-fields");
    assert_eq!(
        value(&index_at(&s, &dir, after(1), None).unwrap(), 1)["excluded"][0]["reason"],
        "missing"
    );
    let other = Address::new_from_array([9; 32]).to_string();
    let end = good["end"].as_i64().unwrap();
    let cases = [
        ("version", json!(99), "version"),
        ("version", json!("1"), "version"),
        ("cluster", json!("mainnet"), "cluster"),
        ("program_id", json!(other), "program_id"),
        ("window", json!(other), "window"),
        ("day", json!(1), "day"),
        ("day", json!(0.0), "day"),
        ("start", json!(1), "start"),
        ("end", json!(end + 1), "end"),
        ("end", json!(end.to_string()), "end"),
        ("slot", json!("7"), "no slot"),
        ("slot", json!(-1), "no slot"),
        ("clock", json!("7"), "no clock"),
        ("extra", json!(1), "extra fields"),
    ];
    for (field, wrong, want) in cases {
        let mut v = good.clone();
        v[field] = wrong.clone();
        std::fs::write(&path, v.to_string()).unwrap();
        let label = format!("{field} = {wrong}");
        let e = index_at(&s, &dir, after(1), None).unwrap_err().to_string();
        assert!(e.contains(want), "{label}: {e}");
        let mut absent = Fake::new(after(1));
        let e = check_stubs(&mut absent, &s, &dir).unwrap_err().to_string();
        assert!(e.contains(want), "check, {label}: {e}");
        assert_eq!(
            absent.calls, 0,
            "check read the chain for a bad stub: {label}"
        );
    }
    for field in [
        "version",
        "cluster",
        "program_id",
        "window",
        "day",
        "start",
        "end",
        "slot",
        "clock",
    ] {
        let mut v = good.clone();
        v.as_object_mut().unwrap().remove(field);
        std::fs::write(&path, v.to_string()).unwrap();
        assert!(index_at(&s, &dir, after(1), None).is_err(), "no {field}");
    }
}

#[test]
fn test_a_second_writer_on_the_directory_fails_and_changes_nothing() {
    let s = series();
    let dir = scratch("two-writers");
    let mut f = Fake::new(after(1)).put(0, &Fill::full());
    std::fs::create_dir_all(&dir).unwrap();
    let held = std::fs::File::open(&dir).unwrap();
    held.try_lock().unwrap();
    let e = series_sync(&mut f, &s, &dir).unwrap_err().to_string();
    assert!(e.contains("another series-sync"), "{e}");
    assert_eq!(f.calls, 0, "it read the chain without the lock");
    assert!(files(&dir).is_empty(), "{:?}", files(&dir));
    drop(held);
    series_sync(&mut f, &s, &dir).unwrap();
    assert!(file_of(&s, &dir, 0).exists());
}

#[test]
fn test_a_file_appearing_during_the_export_is_not_overwritten() {
    let s = series();
    let dir = scratch("race");
    let path = file_of(&s, &dir, 0);
    let mut f = Fake::new(after(1)).put(0, &Fill::full());
    f.race = Some((path.clone(), "{\"other\": \"writer\"}".to_owned()));
    std::fs::create_dir_all(&dir).unwrap();
    let e = series_sync(&mut f, &s, &dir).unwrap_err().to_string();
    assert!(e.contains("not overwritten"), "{e}");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "{\"other\": \"writer\"}"
    );
    assert!(!dir
        .join(s.file_name(0).unwrap().replace(".json", ".json.tmp"))
        .exists());
}

#[test]
fn test_an_existing_conflicting_file_is_kept_and_named_by_the_index() {
    let (s, dir) = synced("conflict", &[(0, Fill::full())], 1);
    let path = file_of(&s, &dir, 0);
    std::fs::write(&path, "{\"other\": \"writer\"}").unwrap();
    let mut f = Fake::new(after(1)).put(0, &Fill::full());
    series_sync(&mut f, &s, &dir).unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "{\"other\": \"writer\"}"
    );
    let e = index_at(&s, &dir, after(1), None).unwrap_err().to_string();
    assert!(e.contains(&s.file_name(0).unwrap()), "{e}");
}

#[test]
fn test_a_file_outside_the_series_is_rejected() {
    let (s, dir) = synced("outside", &[(0, Fill::full())], 1);
    for name in ["0000-wrongaddress.json", "stray.json", "0009-.json"] {
        std::fs::write(dir.join(name), "{}").unwrap();
        let e = index_at(&s, &dir, after(1), None).unwrap_err().to_string();
        assert!(e.contains(name), "{e}");
        std::fs::remove_file(dir.join(name)).unwrap();
    }
}

#[test]
fn test_a_stub_read_before_end_is_rejected_offline() {
    let (s, dir) = synced("old-stub", &[], 1);
    let path = file_of(&s, &dir, 0);
    let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["kind"], "missing");
    v["clock"] = json!(FIRST + 5);
    std::fs::write(&path, v.to_string()).unwrap();
    let e = index_at(&s, &dir, after(1), None).unwrap_err().to_string();
    assert!(e.contains("before its end"), "{e}");
}

#[test]
fn test_check_fails_on_a_stub_whose_account_exists() {
    let (s, dir) = synced("check", &[], 1);
    let mut absent = Fake::new(after(1));
    assert_eq!(check_stubs(&mut absent, &s, &dir).unwrap(), 1);
    let before = files(&dir);
    let mut present = Fake::new(after(1)).put(0, &Fill::full());
    let e = check_stubs(&mut present, &s, &dir).unwrap_err().to_string();
    assert!(e.contains("the account exists"), "{e}");
    assert_eq!(files(&dir), before, "--check wrote something");
}

#[test]
fn test_a_window_of_another_owner_fails_the_sync() {
    let s = series();
    let dir = scratch("owner");
    let mut f = Fake::new(after(1)).put(0, &Fill::full());
    f.owner = Some(Address::new_from_array([9; 32]));
    let e = series_sync(&mut f, &s, &dir).unwrap_err().to_string();
    assert!(e.contains("not owned"), "{e}");
    assert!(!file_of(&s, &dir, 0).exists());
}

#[test]
fn test_the_committed_series_file_parses_to_the_live_addresses() {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../docs/index/series/sol-usd-hourly.json");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let s = Series::parse(&v).unwrap();
    assert_eq!(s.id, "sol-usd-hourly");
    assert_eq!(s.first_start, FIRST);
    assert_eq!(
        s.address(2).unwrap().to_string(),
        "5BaxDUhESZeF69osARDre7fomJbqmuhLUFk6PAvmyGgu"
    );
}

#[test]
fn test_a_record_whose_bytes_are_another_feeds_is_rejected() {
    let s = series();
    let mut other = s.clone();
    other.feed = [3; 32];
    // The other feed's window of the same day, exported honestly, then claimed for this series.
    let address = other.address(0).unwrap();
    let mut f = Fake::new(after(1));
    f.windows
        .insert(address.to_string(), build(&other, 0, &Fill::full()));
    let mut forged = record::export(&mut f, &s.program_id, &address, "devnet").unwrap();
    forged["window"] = json!(s.address(0).unwrap().to_string());
    let dir = scratch("other-feed");
    std::fs::write(file_of(&s, &dir, 0), forged.to_string()).unwrap();
    let e = index_at(&s, &dir, after(1), None).unwrap_err().to_string();
    assert!(e.contains("spec is not this day's"), "{e}");
}

/// The two real devnet days the dashboard's generator is tested on (`scripts/tests/
/// dashboard-section.mjs`). If this fails because the output changed on purpose, regenerate
/// `history.json` with `vol-index --history --json`, and the generator's tests then say whether
/// it still reads what it needs.
#[test]
fn test_the_dashboard_fixture_is_what_the_cli_prints_for_its_records() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let read = |path: PathBuf| -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    };
    let series = Series::parse(&read(repo.join("docs/index/series/sol-usd-hourly.json"))).unwrap();
    let fixture = repo.join("docs/dashboard/fixtures/vol-index");
    let records = fixture.join("records");
    let as_of = resolve_as_of(&records, None).unwrap();
    assert_eq!(
        history(&series, &records, as_of).unwrap(),
        read(fixture.join("history.json"))
    );
}
