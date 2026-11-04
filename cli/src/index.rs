//! The rolling realized volatility index (`docs/PLAN-phase2.md`, P1).
//!
//! A [`Series`] names a feed, an anchor hour, an interval and a first start. Day `d` is the
//! window starting at `first_start + d × 86400`, one day long, at the address derived from
//! that spec, so no list of windows exists and nobody chooses which windows enter.
//!
//! [`series_sync`] keeps one file per day in a directory: an I7 record for a final window, a
//! `missing` stub for a day nobody created. Nothing else is permanent: a day with no file is
//! `open`, and `sync.json` holds the chain time of the last run ([`SyncState`]).
//!
//! [`index_at`] and [`history`] read only those files. For `k` in {1, 7, 30} days ending at day
//! `D`, `RV_k = Σ sum_sq × YEAR_SECONDS / Σ span` over the usable windows, through
//! [`LnKernel::annualize`] on the summed raw `sum_sq`, so `RV_1` of a finalized window equals
//! its stored realized variance. `σ_k = sqrt(RV_k)`. Both round down.

use std::path::{Path, PathBuf};
use std::time::Duration;

use optfix::Rounding;
use serde_json::{json, Value};
use varswap_core::{LnKernel, ReturnKernel, SumSq, Window, WindowState};
use varswap_sdk::pda::{self, WindowKey};
use varswap_sdk::Address;

use crate::record::{self, History};
use crate::{format_var, AccountInfo, CliError};

/// One day, in seconds.
const DAY: u32 = 86_400;
/// The window lengths the index publishes, in days.
pub const PERIODS: [u32; 3] = [1, 7, 30];
/// A `sample` landing this long after its bucket's boundary is late: the watcher's `--grace`.
pub const LATE_GRACE_SECONDS: i64 = 600;
/// An export that would be `unknown` is retried this long after `end`, in case the node is only
/// lagging. A retry policy, not a claim about how long an RPC retains history.
pub const UNKNOWN_RETRY_SECONDS: i64 = 3 * 86_400;
/// The state file's name in the records directory.
pub const SYNC_FILE: &str = "sync.json";
/// The stub format version.
const STUB_VERSION: u64 = 1;
/// The largest result `σ` can have: `sqrt(2^128 × 2^64)` in raw units.
const SIGMA_BOUND: u128 = 1 << 96;

fn bad(what: impl Into<String>) -> CliError {
    CliError::Record(what.into())
}

fn overflow() -> CliError {
    bad("arithmetic overflow")
}

/// Days since 1970-01-01 of a civil date (Hinnant's `days_from_civil`), `None` on overflow.
fn days_from_civil(year: i64, month: i64, day: i64) -> Option<i64> {
    let y = if month <= 2 {
        year.checked_sub(1)?
    } else {
        year
    };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let shifted = if month > 2 {
        month.checked_sub(3)?
    } else {
        month.checked_add(9)?
    };
    let doy = 153i64
        .checked_mul(shifted)?
        .checked_add(2)?
        .div_euclid(5)
        .checked_add(day)?
        .checked_sub(1)?;
    let doe = yoe
        .checked_mul(365)?
        .checked_add(yoe.div_euclid(4))?
        .checked_sub(yoe.div_euclid(100))?
        .checked_add(doy)?;
    era.checked_mul(146_097)?
        .checked_add(doe)?
        .checked_sub(719_468)
}

/// A civil date from days since 1970-01-01 (Hinnant's `civil_from_days`), `None` on overflow.
fn civil_from_days(days: i64) -> Option<(i64, i64, i64)> {
    let z = days.checked_add(719_468)?;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = doe
        .checked_sub(doe.div_euclid(1_460))?
        .checked_add(doe.div_euclid(36_524))?
        .checked_sub(doe.div_euclid(146_096))?
        .div_euclid(365);
    let doy = doe.checked_sub(
        yoe.checked_mul(365)?
            .checked_add(yoe.div_euclid(4))?
            .checked_sub(yoe.div_euclid(100))?,
    )?;
    let mp = doy.checked_mul(5)?.checked_add(2)?.div_euclid(153);
    let day = doy
        .checked_sub(mp.checked_mul(153)?.checked_add(2)?.div_euclid(5))?
        .checked_add(1)?;
    let month = if mp < 10 {
        mp.checked_add(3)?
    } else {
        mp.checked_sub(9)?
    };
    let year = yoe
        .checked_add(era.checked_mul(400)?)?
        .checked_add(i64::from(month <= 2))?;
    Some((year, month, day))
}

/// Parses `YYYY-MM-DDTHH:MM:SSZ`, strictly: no offset, no fraction.
///
/// # Errors
///
/// `CliError::Usage` naming the text.
pub fn parse_time(text: &str) -> Result<i64, CliError> {
    let parsed = || -> Option<i64> {
        let (date, time) = text.strip_suffix('Z')?.split_once('T')?;
        let mut d = date.split('-');
        let (year, month, day) = (d.next()?, d.next()?, d.next()?);
        let mut t = time.split(':');
        let (hour, minute, second) = (t.next()?, t.next()?, t.next()?);
        let widths = [
            (year, 4),
            (month, 2),
            (day, 2),
            (hour, 2),
            (minute, 2),
            (second, 2),
        ];
        if d.next().is_some()
            || t.next().is_some()
            || widths
                .iter()
                .any(|(s, w)| s.len() != *w || !s.bytes().all(|b| b.is_ascii_digit()))
        {
            return None;
        }
        let (year, month, day): (i64, i64, i64) =
            (year.parse().ok()?, month.parse().ok()?, day.parse().ok()?);
        let (hour, minute, second): (i64, i64, i64) = (
            hour.parse().ok()?,
            minute.parse().ok()?,
            second.parse().ok()?,
        );
        if !(1..=12).contains(&month) || hour > 23 || minute > 59 || second > 59 {
            return None;
        }
        let days = days_from_civil(year, month, day)?;
        if civil_from_days(days)? != (year, month, day) {
            return None;
        }
        days.checked_mul(i64::from(DAY))?
            .checked_add(hour.checked_mul(3_600)?)?
            .checked_add(minute.checked_mul(60)?)?
            .checked_add(second)
    };
    parsed()
        .ok_or_else(|| CliError::Usage(format!("{text} is not a time like 2026-11-02T17:00:00Z")))
}

/// Formats a Unix time as `YYYY-MM-DDTHH:MM:SSZ`.
///
/// # Errors
///
/// `CliError::Record` on overflow.
pub fn format_time(t: i64) -> Result<String, CliError> {
    let day = i64::from(DAY);
    let (year, month, d) = civil_from_days(t.div_euclid(day)).ok_or_else(overflow)?;
    let s = t.rem_euclid(day);
    let (hour, rest) = (s.div_euclid(3_600), s.rem_euclid(3_600));
    let (minute, second) = (rest.div_euclid(60), rest.rem_euclid(60));
    Ok(format!(
        "{year:04}-{month:02}-{d:02}T{hour:02}:{minute:02}:{second:02}Z"
    ))
}

/// What defines an index series (`docs/index/series/*.json`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Series {
    /// The series' name.
    pub id: String,
    /// Pyth feed id.
    pub feed: [u8; 32],
    /// Bucket interval, seconds.
    pub interval: u32,
    /// Start of day 0.
    pub first_start: i64,
    /// The varswap program.
    pub program_id: Address,
    /// The cluster's name, written into the records.
    pub cluster: String,
}

impl Series {
    /// Parses a series file's JSON.
    ///
    /// # Errors
    ///
    /// `CliError::Usage` naming the field, or a `first_start` not on the anchor hour.
    pub fn parse(v: &Value) -> Result<Self, CliError> {
        let field = |k: &str| {
            v.get(k)
                .and_then(Value::as_str)
                .ok_or_else(|| CliError::Usage(format!("the series has no string field {k}")))
        };
        let number = |k: &str| {
            v.get(k)
                .and_then(Value::as_u64)
                .ok_or_else(|| CliError::Usage(format!("the series has no number field {k}")))
        };
        let first_start = parse_time(field("first_start")?)?;
        let anchor = i64::try_from(number("anchor_hour")?)
            .ok()
            .and_then(|h| h.checked_mul(3_600))
            .ok_or_else(|| CliError::Usage("anchor_hour is out of range".to_owned()))?;
        if first_start.rem_euclid(i64::from(DAY)) != anchor {
            return Err(CliError::Usage(
                "first_start is not on the series' anchor hour".to_owned(),
            ));
        }
        let interval = u32::try_from(number("interval")?)
            .ok()
            .filter(|i| *i > 0 && DAY.is_multiple_of(*i))
            .ok_or_else(|| CliError::Usage("interval does not divide a day".to_owned()))?;
        Ok(Self {
            id: field("id")?.to_owned(),
            feed: crate::parse_feed_id(field("feed")?)?,
            interval,
            first_start,
            program_id: crate::parse_address(field("program_id")?)?,
            cluster: field("cluster")?.to_owned(),
        })
    }

    /// Start of day `day`.
    ///
    /// # Errors
    ///
    /// `CliError::Record` on overflow.
    pub fn start(&self, day: u32) -> Result<i64, CliError> {
        i64::from(day)
            .checked_mul(i64::from(DAY))
            .and_then(|s| s.checked_add(self.first_start))
            .ok_or_else(overflow)
    }

    /// End of day `day`.
    ///
    /// # Errors
    ///
    /// `CliError::Record` on overflow.
    pub fn end(&self, day: u32) -> Result<i64, CliError> {
        self.start(day)?
            .checked_add(i64::from(DAY))
            .ok_or_else(overflow)
    }

    /// The window address of day `day`.
    ///
    /// # Errors
    ///
    /// `CliError::Record` on overflow.
    pub fn address(&self, day: u32) -> Result<Address, CliError> {
        let key = WindowKey {
            feed: self.feed,
            start: self.start(day)?,
            end: self.end(day)?,
            interval: self.interval,
        };
        Ok(pda::window(&self.program_id, &key).0)
    }

    /// The last day whose `end` is at or before `as_of`, `None` if no day has ended.
    #[must_use]
    pub fn last_ended_day(&self, as_of: i64) -> Option<u32> {
        let elapsed = as_of.checked_sub(self.first_start)?;
        let days = elapsed.div_euclid(i64::from(DAY));
        u32::try_from(days.checked_sub(1)?).ok()
    }

    /// The file of day `day` in a records directory.
    ///
    /// # Errors
    ///
    /// `CliError::Record` on overflow.
    pub fn file_name(&self, day: u32) -> Result<String, CliError> {
        Ok(format!("{day:04}-{}.json", self.address(day)?))
    }
}

/// An account and the chain's clock, read at finalized commitment in one snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// The snapshot's slot.
    pub slot: u64,
    /// The `Clock` sysvar's `unix_timestamp` in that snapshot.
    pub clock: i64,
    /// The account; `None` only for an explicit `null` from the node.
    pub account: Option<AccountInfo>,
}

/// What [`series_sync`] needs from a cluster beyond a record's [`History`].
pub trait SeriesCluster: History {
    /// One `getMultipleAccounts([address, Clock])` at finalized commitment.
    ///
    /// # Errors
    ///
    /// The cluster cannot answer, or its answer has no clock.
    fn snapshot(&mut self, address: &Address) -> Result<Snapshot, CliError>;
}

/// The pace of a cluster's calls and the retry of its transient errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pace {
    /// The least time between two calls.
    pub spacing: Duration,
    /// Attempts after the first, on [`CliError::RpcTransient`] only.
    pub retries: u32,
    /// The wait before the first retry; it doubles each time.
    pub backoff: Duration,
}

impl Pace {
    /// `rate` requests per second, with four retries from 5 s (5, 10, 20, 40).
    ///
    /// # Errors
    ///
    /// `CliError::Usage` if `rate` is zero.
    pub fn per_second(rate: u32) -> Result<Self, CliError> {
        let spacing = Duration::from_secs(1)
            .checked_div(rate)
            .ok_or_else(|| CliError::Usage("--rate must be at least 1".to_owned()))?;
        Ok(Self {
            spacing,
            retries: 4,
            backoff: Duration::from_secs(5),
        })
    }
}

/// A cluster behind a [`Pace`]. `sleep` is injected so the tests do not wait.
pub struct Paced<C, S> {
    inner: C,
    pace: Pace,
    sleep: S,
    called: bool,
}

impl<C, S: FnMut(Duration)> Paced<C, S> {
    /// Wraps `inner`.
    pub const fn new(inner: C, pace: Pace, sleep: S) -> Self {
        Self {
            inner,
            pace,
            sleep,
            called: false,
        }
    }

    /// The wrapped cluster.
    pub const fn inner(&self) -> &C {
        &self.inner
    }

    fn call<T>(&mut self, mut f: impl FnMut(&mut C) -> Result<T, CliError>) -> Result<T, CliError> {
        if self.called {
            (self.sleep)(self.pace.spacing);
        }
        self.called = true;
        let mut wait = self.pace.backoff;
        let mut left = self.pace.retries;
        loop {
            match f(&mut self.inner) {
                Err(CliError::RpcTransient(_)) if left > 0 => {
                    (self.sleep)(wait);
                    wait = wait.saturating_mul(2);
                    left = left.saturating_sub(1);
                }
                other => return other,
            }
        }
    }
}

impl<C: History, S: FnMut(Duration)> History for Paced<C, S> {
    fn signatures(
        &mut self,
        address: &Address,
        before: Option<&str>,
    ) -> Result<Vec<String>, CliError> {
        self.call(|c| c.signatures(address, before))
    }

    fn transaction(&mut self, signature: &str) -> Result<Option<Value>, CliError> {
        self.call(|c| c.transaction(signature))
    }

    fn account_at(&mut self, address: &Address) -> Result<(u64, Option<AccountInfo>), CliError> {
        self.call(|c| c.account_at(address))
    }
}

impl<C: SeriesCluster, S: FnMut(Duration)> SeriesCluster for Paced<C, S> {
    fn snapshot(&mut self, address: &Address) -> Result<Snapshot, CliError> {
        self.call(|c| c.snapshot(address))
    }
}

/// `<dir>/sync.json`: the chain time of the last run. State, not a record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncState {
    /// The finalized clock; never moves backwards.
    pub as_of: i64,
    /// The slot of the snapshot that gave `as_of`.
    pub slot: u64,
}

/// Reads `<dir>/sync.json`, `None` if there is none.
///
/// # Errors
///
/// The file exists and cannot be read or is malformed.
pub fn read_sync(dir: &Path) -> Result<Option<SyncState>, CliError> {
    let path = dir.join(SYNC_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(bad(format!("{}: {e}", path.display()))),
    };
    let v: Value =
        serde_json::from_str(&text).map_err(|e| bad(format!("{}: {e}", path.display())))?;
    let read = || {
        Some(SyncState {
            as_of: v.get("as_of")?.as_i64()?,
            slot: v.get("slot")?.as_u64()?,
        })
    };
    read()
        .map(Some)
        .ok_or_else(|| bad(format!("{}: no as_of or slot", path.display())))
}

/// Writes through a temporary file, so a reader never sees half a record.
fn write_json(path: &Path, v: &Value) -> Result<(), CliError> {
    let text = serde_json::to_string_pretty(v).map_err(|e| bad(e.to_string()))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, format!("{text}\n"))
        .and_then(|()| std::fs::rename(&tmp, path))
        .map_err(|e| bad(format!("{}: {e}", path.display())))
}

/// Why a day has no file yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenReason {
    /// The account exists and can still fill or degrade.
    NotFinal,
    /// The node answered `null` from a snapshot before the day's `end`: that proves nothing.
    SnapshotBeforeEnd,
    /// The export would be `unknown` and the retry period has not passed.
    HistoryUnproven,
}

impl OpenReason {
    const fn name(self) -> &'static str {
        match self {
            Self::NotFinal => "not_final",
            Self::SnapshotBeforeEnd => "snapshot_before_end",
            Self::HistoryUnproven => "history_unproven",
        }
    }
}

/// What one run wrote.
enum Outcome {
    Written(String),
    Open(OpenReason),
}

fn window_final(data: &[u8]) -> Result<bool, CliError> {
    Window::inspect(data)
        .map(|w| w.state.is_final())
        .map_err(|e| bad(format!("not a valid window: {e:?}")))
}

fn stub(
    series: &Series,
    day: u32,
    address: &Address,
    snapshot: &Snapshot,
) -> Result<Value, CliError> {
    Ok(json!({
        "version": STUB_VERSION,
        "kind": "missing",
        "cluster": series.cluster,
        "program_id": series.program_id.to_string(),
        "window": address.to_string(),
        "day": day,
        "start": series.start(day)?,
        "end": series.end(day)?,
        "slot": snapshot.slot,
        "clock": snapshot.clock,
    }))
}

fn sync_day<C: SeriesCluster>(
    c: &mut C,
    series: &Series,
    day: u32,
    path: &Path,
) -> Result<Outcome, CliError> {
    let address = series.address(day)?;
    let end = series.end(day)?;
    let snapshot = c.snapshot(&address)?;
    let Some(account) = &snapshot.account else {
        // `create_window` needs `start > now` on the chain's clock, this finalized snapshot is
        // already past `end > start`, and no instruction closes a window: the absence is final.
        if snapshot.clock < end {
            return Ok(Outcome::Open(OpenReason::SnapshotBeforeEnd));
        }
        write_json(path, &stub(series, day, &address, &snapshot)?)?;
        return Ok(Outcome::Written("missing".to_owned()));
    };
    if !window_final(&account.data)? {
        return Ok(Outcome::Open(OpenReason::NotFinal));
    }
    let bundle = record::export(c, &series.program_id, &address, &series.cluster)?;
    let label = bundle
        .pointer("/derived/label")
        .and_then(Value::as_str)
        .unwrap_or("?")
        .to_owned();
    if label == "unknown" {
        let retry_until = end
            .checked_add(UNKNOWN_RETRY_SECONDS)
            .ok_or_else(overflow)?;
        if snapshot.clock < retry_until {
            return Ok(Outcome::Open(OpenReason::HistoryUnproven));
        }
        // The history is what is missing, so it cannot be checked against the cluster. The
        // record enters no value, and its label says so.
        record::verify_offline(&bundle)?;
    } else {
        record::verify_rpc(c, &bundle)?;
    }
    write_json(path, &bundle)?;
    Ok(Outcome::Written(label))
}

/// Brings `dir` up to date with the cluster: see the module note.
///
/// A failure stops the run with what it wrote so far kept, and a later run resumes. `sync.json`
/// is written either way.
///
/// # Errors
///
/// The cluster cannot answer after the pace's retries, a record does not verify, or a file
/// cannot be written.
pub fn series_sync<C: SeriesCluster>(
    c: &mut C,
    series: &Series,
    dir: &Path,
) -> Result<Value, CliError> {
    std::fs::create_dir_all(dir).map_err(|e| bad(format!("{}: {e}", dir.display())))?;
    let first = c.snapshot(&series.address(0)?)?;
    let state = match read_sync(dir)? {
        Some(previous) if previous.as_of > first.clock => previous,
        _ => SyncState {
            as_of: first.clock,
            slot: first.slot,
        },
    };
    let last_day = series.last_ended_day(state.as_of);
    let (mut written, mut open) = (Vec::new(), Vec::new());
    let mut failure = None;
    for day in last_day.map_or(0..0, |d| 0..d.saturating_add(1)) {
        let path = dir.join(series.file_name(day)?);
        if path.exists() {
            continue;
        }
        match sync_day(c, series, day, &path) {
            Ok(Outcome::Written(label)) => written.push(json!({ "day": day, "label": label })),
            Ok(Outcome::Open(reason)) => {
                open.push(json!({ "day": day, "reason": reason.name() }));
            }
            Err(e) => {
                failure = Some(e);
                break;
            }
        }
    }
    let summary = json!({
        "series": series.id,
        "as_of": state.as_of,
        "as_of_utc": format_time(state.as_of)?,
        "slot": state.slot,
        "last_day": last_day,
        "open": open,
    });
    write_json(&dir.join(SYNC_FILE), &summary)?;
    match failure {
        Some(e) => Err(e),
        None => Ok(json!({ "sync": summary, "written": written })),
    }
}

/// Re-reads every `missing` stub in `dir`: writes nothing, and fails if any account exists.
///
/// # Errors
///
/// A stub whose window exists, or the cluster's error. Returns how many stubs were checked.
pub fn check_stubs<C: SeriesCluster>(
    c: &mut C,
    series: &Series,
    dir: &Path,
) -> Result<u32, CliError> {
    let mut checked = 0u32;
    for (day, file) in series_files(series, dir)? {
        let v = read_file(&file)?;
        if v.get("kind").and_then(Value::as_str) != Some("missing") {
            continue;
        }
        let snapshot = c.snapshot(&series.address(day)?)?;
        if snapshot.account.is_some() {
            return Err(bad(format!(
                "{}: a missing stub, and the account exists",
                file.display()
            )));
        }
        checked = checked.saturating_add(1);
    }
    Ok(checked)
}

fn read_file(path: &Path) -> Result<Value, CliError> {
    let text =
        std::fs::read_to_string(path).map_err(|e| bad(format!("{}: {e}", path.display())))?;
    serde_json::from_str(&text).map_err(|e| bad(format!("{}: {e}", path.display())))
}

/// Every JSON file in `dir` but `sync.json`, as (day, path), sorted. A file that is not named
/// for a day of this series is an error, never skipped.
fn series_files(series: &Series, dir: &Path) -> Result<Vec<(u32, PathBuf)>, CliError> {
    let entries = std::fs::read_dir(dir).map_err(|e| bad(format!("{}: {e}", dir.display())))?;
    let mut files = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|e| bad(format!("{}: {e}", dir.display())))?
            .path();
        if path.extension().is_none_or(|x| x != "json")
            || path.file_name().is_some_and(|n| n == SYNC_FILE)
        {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_owned();
        let day = name
            .split_once('-')
            .and_then(|(d, _)| d.parse::<u32>().ok())
            .ok_or_else(|| bad(format!("{name}: not a file of this series")))?;
        if series.file_name(day)? != name {
            return Err(bad(format!(
                "{name}: not this series' window for day {day}"
            )));
        }
        files.push((day, path));
    }
    files.sort();
    Ok(files)
}

/// One day of the series, as its file proves it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Day {
    /// No file: no final record yet.
    Open,
    /// No window was created for this day.
    Missing,
    /// Final, with no realized variance.
    Degraded { empty_buckets: u32 },
    /// Final, with a history that does not prove its runs.
    Unknown,
    /// Finalized and proven.
    Usable(Usable),
}

/// What a usable window adds to a value.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Usable {
    sum_sq: u128,
    span: u64,
    compressed: bool,
    compressed_buckets: u32,
    compressed_runs: u32,
    longest_run: u32,
    late_samples: u32,
}

fn str_field<'a>(v: &'a Value, pointer: &str, name: &str) -> Result<&'a str, CliError> {
    v.pointer(pointer)
        .and_then(Value::as_str)
        .ok_or_else(|| bad(format!("{name}: {pointer} is missing")))
}

fn u32_field(v: &Value, pointer: &str, name: &str) -> Result<u32, CliError> {
    v.pointer(pointer)
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| bad(format!("{name}: {pointer} is missing")))
}

fn load_stub(series: &Series, day: u32, v: &Value, name: &str) -> Result<Day, CliError> {
    let end = series.end(day)?;
    let clock = v
        .get("clock")
        .and_then(Value::as_i64)
        .ok_or_else(|| bad(format!("{name}: a stub with no clock")))?;
    if v.get("day").and_then(Value::as_u64) != Some(u64::from(day)) {
        return Err(bad(format!("{name}: a stub for another day")));
    }
    if clock < end {
        return Err(bad(format!(
            "{name}: a missing stub read at clock {clock}, before its end {end}"
        )));
    }
    Ok(Day::Missing)
}

fn late_samples(series: &Series, day: u32, v: &Value, name: &str) -> Result<u32, CliError> {
    let start = series.start(day)?;
    let mut late = 0u32;
    let transactions = v
        .pointer("/history/transactions")
        .and_then(Value::as_array)
        .ok_or_else(|| bad(format!("{name}: no transaction list")))?;
    for t in transactions {
        let Some(block_time) = t.get("block_time").and_then(Value::as_i64) else {
            continue;
        };
        for e in t
            .get("samples")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let first = u32_field(e, "/first", name)?;
            let boundary = i64::from(first)
                .checked_mul(i64::from(series.interval))
                .and_then(|o| o.checked_add(start))
                .and_then(|b| b.checked_add(LATE_GRACE_SECONDS))
                .ok_or_else(overflow)?;
            if block_time > boundary {
                late = late.saturating_add(1);
            }
        }
    }
    Ok(late)
}

fn load_record(series: &Series, day: u32, v: &Value, name: &str) -> Result<Day, CliError> {
    let derived = record::verify_offline(v).map_err(|e| bad(format!("{name}: {e}")))?;
    if str_field(v, "/window", name)? != series.address(day)?.to_string() {
        return Err(bad(format!("{name}: a record of another window")));
    }
    if str_field(v, "/program_id", name)? != series.program_id.to_string()
        || str_field(v, "/cluster", name)? != series.cluster
    {
        return Err(bad(format!(
            "{name}: a record of another program or cluster"
        )));
    }
    // `verify_offline` checks the bytes against themselves; here they must be this day's spec.
    let data = record::account_bytes(v)?;
    let info = Window::inspect(&data).map_err(|e| bad(format!("{name}: {e:?}")))?;
    if info.spec.feed().to_bytes() != series.feed
        || info.spec.start().get() != series.start(day)?
        || info.spec.end().get() != series.end(day)?
        || info.spec.interval().get() != series.interval
    {
        return Err(bad(format!("{name}: the account's spec is not this day's")));
    }
    let label = derived.get("label").and_then(Value::as_str).unwrap_or("?");
    Ok(match (info.state, label) {
        (_, "unknown") => Day::Unknown,
        (WindowState::Degraded, _) => {
            let (buckets, filled) = (
                u32_field(&derived, "/buckets", name)?,
                u32_field(&derived, "/filled", name)?,
            );
            Day::Degraded {
                empty_buckets: buckets.checked_sub(filled).ok_or_else(overflow)?,
            }
        }
        (WindowState::Finalized, "complete" | "compressed") => {
            let mut usable = Usable {
                sum_sq: str_field(&derived, "/sum_sq_raw", name)?
                    .parse()
                    .map_err(|_| bad(format!("{name}: sum_sq_raw is not a number")))?,
                span: u64::from(DAY),
                compressed: label == "compressed",
                compressed_buckets: 0,
                compressed_runs: 0,
                longest_run: 1,
                late_samples: late_samples(series, day, v, name)?,
            };
            let runs = derived
                .get("runs")
                .and_then(Value::as_array)
                .ok_or_else(|| bad(format!("{name}: a proven record with no runs")))?;
            for run in runs {
                let (a, b) = (u32_field(run, "/0", name)?, u32_field(run, "/1", name)?);
                let extra = b.checked_sub(a).ok_or_else(overflow)?;
                if extra > 0 {
                    usable.compressed_runs = usable.compressed_runs.saturating_add(1);
                    usable.compressed_buckets = usable.compressed_buckets.saturating_add(extra);
                    usable.longest_run = usable.longest_run.max(extra.saturating_add(1));
                }
            }
            Day::Usable(usable)
        }
        (state, label) => {
            return Err(bad(format!(
                "{name}: a record in state {state:?} with label {label}"
            )))
        }
    })
}

/// Every day from 0 to `last_day`. Every file in the directory is verified offline, those past
/// `last_day` included, and one that fails is an error naming it, never a silent exclusion.
fn load_days(series: &Series, dir: &Path, last_day: u32) -> Result<Vec<Day>, CliError> {
    let count = usize::try_from(last_day)
        .ok()
        .and_then(|d| d.checked_add(1))
        .ok_or_else(overflow)?;
    let mut days = vec![Day::Open; count];
    for (day, path) in series_files(series, dir)? {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_owned();
        let v = read_file(&path)?;
        let loaded = if v.get("kind").and_then(Value::as_str) == Some("missing") {
            load_stub(series, day, &v, &name)?
        } else {
            load_record(series, day, &v, &name)?
        };
        if let Some(slot) = usize::try_from(day).ok().and_then(|d| days.get_mut(d)) {
            *slot = loaded;
        }
    }
    Ok(days)
}

/// `sqrt(var)` in Q64.64, rounded down: the largest `x` with `x² ≤ var_raw × 2^64`.
///
/// # Errors
///
/// `CliError::Record` if the wide product fails, which the bound below rules out.
pub fn sigma_raw(var_raw: u128) -> Result<u128, CliError> {
    let (mut low, mut high) = (0u128, SIGMA_BOUND);
    while high.checked_sub(low).ok_or_else(overflow)? > 1 {
        let mid = low
            .checked_add(high.checked_sub(low).ok_or_else(overflow)? >> 1)
            .ok_or_else(overflow)?;
        // `mid < 2^96`, so `mid² / 2^64 < 2^128` fits. Up: `ceil(x² / 2^64) ≤ v` iff
        // `x² ≤ v × 2^64`, exactly.
        let square =
            optfix::wide::mul_shr(mid, mid, 64, Rounding::Up).map_err(|e| bad(format!("{e:?}")))?;
        if square <= var_raw {
            low = mid;
        } else {
            high = mid;
        }
    }
    Ok(low)
}

fn value(series: &Series, days: &[Day], last: u32, k: u32) -> Result<Value, CliError> {
    let first = i64::from(last)
        .checked_sub(i64::from(k))
        .and_then(|f| f.checked_add(1))
        .ok_or_else(overflow)?;
    let before_series = u32::try_from(first.min(0).unsigned_abs()).map_err(|_| overflow())?;
    let mut excluded = Vec::new();
    let (mut sum_sq, mut span) = (0u128, 0u64);
    let (mut usable, mut any_compressed) = (0u32, false);
    let (mut compressed_buckets, mut compressed_runs, mut longest_run) = (0u32, 0u32, 0u32);
    let (mut empty_buckets, mut late) = (0u32, 0u32);
    for day in u32::try_from(first.max(0)).map_err(|_| overflow())?..=last {
        let entry = usize::try_from(day).ok().and_then(|d| days.get(d));
        let reason = match entry {
            None | Some(Day::Open) => "open",
            Some(Day::Missing) => "missing",
            Some(Day::Unknown) => "unknown",
            Some(Day::Degraded { empty_buckets: e }) => {
                empty_buckets = empty_buckets.checked_add(*e).ok_or_else(overflow)?;
                "degraded"
            }
            Some(Day::Usable(u)) => {
                sum_sq = sum_sq.checked_add(u.sum_sq).ok_or_else(overflow)?;
                span = span.checked_add(u.span).ok_or_else(overflow)?;
                usable = usable.saturating_add(1);
                any_compressed |= u.compressed;
                compressed_buckets = compressed_buckets.saturating_add(u.compressed_buckets);
                compressed_runs = compressed_runs.saturating_add(u.compressed_runs);
                longest_run = longest_run.max(u.longest_run);
                late = late.saturating_add(u.late_samples);
                continue;
            }
        };
        excluded.push(json!({
            "day": day,
            "window": series.address(day)?.to_string(),
            "reason": reason,
        }));
    }
    let label = if usable < k.div_ceil(2) {
        "insufficient"
    } else if !excluded.is_empty() || before_series > 0 {
        "partial"
    } else if any_compressed {
        "compressed"
    } else {
        "complete"
    };
    let (rv_raw, rv, sigma_value, sigma_text) = if label == "insufficient" {
        (None, None, None, None)
    } else {
        let var = LnKernel
            .annualize(SumSq::from_raw(sum_sq), span)
            .map_err(|e| bad(format!("{e:?}")))?;
        let sigma = sigma_raw(var)?;
        (
            Some(var.to_string()),
            Some(format_var(var)),
            Some(sigma.to_string()),
            Some(format_var(sigma)),
        )
    };
    Ok(json!({
        "days": k,
        "usable": usable,
        "label": label,
        "rv_raw": rv_raw,
        "rv": rv,
        "sigma_raw": sigma_value,
        "sigma": sigma_text,
        "excluded": excluded,
        "before_series": before_series,
        "missed_samples": compressed_buckets.saturating_add(empty_buckets),
        "compressed_buckets": compressed_buckets,
        "compressed_runs": compressed_runs,
        "longest_run": longest_run,
        "empty_buckets": empty_buckets,
        "late_samples": late,
    }))
}

fn day_entry(series: &Series, days: &[Day], day: u32) -> Result<Value, CliError> {
    let mut values = Vec::new();
    for k in PERIODS {
        values.push(value(series, days, day, k)?);
    }
    Ok(json!({
        "day": day,
        "start": format_time(series.start(day)?)?,
        "end": format_time(series.end(day)?)?,
        "window": series.address(day)?.to_string(),
        "values": values,
    }))
}

/// `as_of` for an offline read: the caller's, or `sync.json`'s. With neither there is no way to
/// know which days have ended, so it is an error rather than a guess from the files present.
///
/// # Errors
///
/// `CliError::Usage` when there is neither, or `sync.json` cannot be read.
pub fn resolve_as_of(dir: &Path, given: Option<i64>) -> Result<i64, CliError> {
    match (given, read_sync(dir)?) {
        (Some(t), _) => Ok(t),
        (None, Some(state)) => Ok(state.as_of),
        (None, None) => Err(CliError::Usage(format!(
            "{} has no {SYNC_FILE}: pass --as-of or run series-sync",
            dir.display()
        ))),
    }
}

/// The values of day `day`, default the last day ended at `as_of`.
///
/// # Errors
///
/// A `day` later than `as_of` allows, or any file that fails its checks.
pub fn index_at(
    series: &Series,
    dir: &Path,
    as_of: i64,
    day: Option<u32>,
) -> Result<Value, CliError> {
    let last = series.last_ended_day(as_of);
    let chosen = match (day, last) {
        (Some(d), Some(l)) if d > l => {
            return Err(CliError::Usage(format!(
                "day {d} has not ended at as_of {as_of}: the last ended day is {l}"
            )))
        }
        (Some(d), None) => {
            return Err(CliError::Usage(format!(
                "day {d} has not ended at as_of {as_of}: no day has"
            )))
        }
        (Some(d), Some(_)) => d,
        (None, Some(l)) => l,
        (None, None) => {
            return Ok(json!({ "series": series.id, "as_of": as_of, "last_day": null }))
        }
    };
    let last = last.unwrap_or(chosen);
    let days = load_days(series, dir, last)?;
    let mut out = day_entry(series, &days, chosen)?;
    if let Some(o) = out.as_object_mut() {
        o.insert("series".to_owned(), json!(series.id));
        o.insert("as_of".to_owned(), json!(as_of));
        o.insert("last_day".to_owned(), json!(last));
    }
    Ok(out)
}

/// One entry per day from 0 to the last day ended at `as_of`, with the series' spec.
///
/// # Errors
///
/// Any file that fails its checks.
pub fn history(series: &Series, dir: &Path, as_of: i64) -> Result<Value, CliError> {
    let last = series.last_ended_day(as_of);
    let mut entries = Vec::new();
    if let Some(last) = last {
        let days = load_days(series, dir, last)?;
        for day in 0..=last {
            entries.push(day_entry(series, &days, day)?);
        }
    }
    Ok(json!({
        "series": {
            "id": series.id,
            "feed": record::hex(&series.feed),
            "interval": series.interval,
            "first_start": format_time(series.first_start)?,
            "program_id": series.program_id.to_string(),
            "cluster": series.cluster,
        },
        "as_of": as_of,
        "as_of_utc": format_time(as_of)?,
        "slot": read_sync(dir)?.map(|s| s.slot),
        "last_day": last,
        "history": entries,
    }))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::panic_in_result_fn,
    clippy::integer_division
)]
mod tests {
    use super::*;

    #[test]
    fn test_time_round_trip_and_rejects() {
        for (text, t) in [
            ("1970-01-01T00:00:00Z", 0),
            ("2026-11-02T17:00:00Z", 1_793_638_800),
            ("2024-02-29T23:59:59Z", 1_709_251_199),
            ("2000-03-01T00:00:00Z", 951_868_800),
        ] {
            assert_eq!(parse_time(text).unwrap(), t, "{text}");
            assert_eq!(format_time(t).unwrap(), text);
        }
        for bad in [
            "2026-11-02",
            "2026-11-02T17:00:00",
            "2026-11-02T17:00:00+00:00",
            "2026-13-02T17:00:00Z",
            "2026-02-30T17:00:00Z",
            "2026-11-02T24:00:00Z",
            "2026-1-02T17:00:00Z",
        ] {
            assert!(parse_time(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn test_time_round_trips_every_day_across_centuries() {
        let mut t = parse_time("1899-12-25T06:30:15Z").unwrap();
        for _ in 0..80_000 {
            assert_eq!(parse_time(&format_time(t).unwrap()).unwrap(), t);
            t += 86_400;
        }
    }

    #[test]
    fn test_sigma_is_the_exact_floor() {
        let one = 1u128 << 64;
        assert_eq!(sigma_raw(0).unwrap(), 0);
        assert_eq!(sigma_raw(one).unwrap(), one);
        assert_eq!(sigma_raw(one >> 2).unwrap(), one >> 1);
        assert_eq!(sigma_raw(one << 2).unwrap(), one << 1);
        assert_eq!(sigma_raw(u128::MAX).unwrap() >> 90, (1 << 6) - 1);
        for v in [
            1u128,
            2,
            3,
            1_000,
            one + 1,
            one - 1,
            123_456_789_012_345_678_901,
            u128::MAX,
        ] {
            let x = sigma_raw(v).unwrap();
            let below = optfix::wide::mul_shr(x, x, 64, Rounding::Up).unwrap();
            assert!(below <= v, "{v}: x² above");
            let above = optfix::wide::mul_shr(x + 1, x + 1, 64, Rounding::Up);
            assert!(above.map_or(true, |a| a > v), "{v}: not the largest x");
        }
    }

    fn series() -> Series {
        Series::parse(&json!({
            "id": "t", "feed": "ef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d",
            "anchor_hour": 17, "interval": 3600, "first_start": "2026-11-02T17:00:00Z",
            "program_id": "BWqsaXvASrbErApP1ovz7vSvBLYqXGaMaZANEYf1xrB9", "cluster": "devnet",
        }))
        .unwrap()
    }

    #[test]
    fn test_series_days_and_addresses() {
        let s = series();
        let first = s.first_start;
        assert_eq!(s.last_ended_day(first), None);
        assert_eq!(s.last_ended_day(first - 1), None);
        assert_eq!(s.last_ended_day(first + 86_399), None);
        assert_eq!(s.last_ended_day(first + 86_400), Some(0));
        assert_eq!(s.last_ended_day(first + 2 * 86_400), Some(1));
        // The live series' first window, derived with no list.
        assert_eq!(
            s.address(0).unwrap().to_string(),
            "7ywXkV5Sa3fGHKhTAcdXqR5jkNg5J7pqvGQg1Hh6XjG1"
        );
        assert_eq!(
            s.address(1).unwrap().to_string(),
            "CjtkCZY1BEzN8MFgrbViGtW9KEUqP6vMKu7CCPRqiATG"
        );
    }

    #[test]
    fn test_series_rejects_a_start_off_its_anchor_and_a_bad_interval() {
        let base = json!({
            "id": "t", "feed": "ef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d",
            "anchor_hour": 16, "interval": 3600, "first_start": "2026-11-02T17:00:00Z",
            "program_id": "BWqsaXvASrbErApP1ovz7vSvBLYqXGaMaZANEYf1xrB9", "cluster": "devnet",
        });
        assert!(Series::parse(&base).is_err());
        let mut v = base;
        v["anchor_hour"] = json!(17);
        v["interval"] = json!(7);
        assert!(Series::parse(&v).is_err());
    }
}
