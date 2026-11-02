//! One Hermes request budget for every varswap process on a host (`docs/PLAN-improvements.md`,
//! I6).
//!
//! The Hermes API key is shared with metalbook; varswap's allocation is `R` requests per minute
//! across all its processes (`docs/governance.md`: 3). [`HermesLimiter`] enforces it on the
//! times requests can reach Hermes, not only on the times they were admitted: every process
//! shares one file of entries, under an exclusive `flock`, one entry per request (retries and
//! requests answered 404 or 429 included), each an upper bound on when that request reached
//! Hermes.
//!
//! - [`HermesLimiter::acquire`] admits a request only if fewer than `R` entries are later than
//!   `now - window`, and records a pending entry at `now + pending` (15 s: the HTTP client's
//!   10 s global timeout plus margin), the latest time the request can arrive if it is sent
//!   within that bound.
//! - [`HermesLimiter::confirm`], called once the request has completed (answered or failed),
//!   lowers the entry to the completion time: the request arrived before it was answered.
//!
//! So a request admitted at `g` is counted from `g` until its arrival bound plus the window,
//! and no `R + 1` requests reach Hermes within one window: at most `R` arrivals in any sliding
//! window, however long the sends take after admission (the drill's SIGSTOP
//! included), provided each send completes within `pending` of its admission. The residual
//! case: a process paused between admission and send for longer than `pending`. Its entry has
//! expired by then, so its late request can share a window with `R` others; `confirm` re-records
//! it at the completion time, and if Hermes answers 429 the block below applies. A process that
//! dies holding a pending entry only delays the others until that entry expires.
//!
//! A 429 blocks every process on that file for the block time (61 s: Hermes asks for 60): no
//! request is admitted until it has passed. Requests admitted before the block was recorded (at
//! most `R − 1` others whose entries overlap the rate-limited one) can still arrive.
//!
//! The files are named after a hash of the API key (64-bit FNV-1a, never the key itself), so two
//! keys never share a budget. Crash safety: the `flock` is held on a separate lock file whose
//! inode never changes, and the state is replaced atomically (written to a temporary file,
//! synced, then renamed over the state file), so a process killed at any point leaves either
//! the old state or the new one. The state ends with an `end` line; a state file without it, or
//! one that cannot be parsed, is read conservatively: a full window and a block from now, which
//! only delays requests. A missing state file is a fresh budget. Entry lines are
//! `<time> <request id>`; a line with a time only (the format before `confirm`) is read as a
//! confirmed entry, while older binaries read the new format conservatively, so every varswap
//! process on a host is upgraded together. The scope is one host: processes on another machine
//! do not see these files.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// varswap's share of the shared key's quota on the timestamp endpoint, per minute.
pub const DEFAULT_REQUESTS_PER_MINUTE: u32 = 3;
/// The sliding window, in milliseconds.
pub const WINDOW_MS: u64 = 60_000;
/// How long a 429 blocks every process, in milliseconds (Hermes asks for 60 s).
pub const BLOCK_MS: u64 = 61_000;
/// How long after admission a request's entry is held at its latest possible arrival time
/// until it is confirmed, in milliseconds: the HTTP client's 10 s global timeout plus margin.
pub const PENDING_MS: u64 = 15_000;

/// The per-process counter behind request ids (the high half is the process id).
static NEXT_SLOT: AtomicU32 = AtomicU32::new(1);

/// 64-bit FNV-1a: a stable, dependency free name for the key's file. Not a secret-preserving
/// hash in the cryptographic sense; it names the file and nothing else.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Milliseconds since the Unix epoch.
fn now_ms() -> io::Result<u64> {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| io::Error::other("system clock before 1970"))?;
    u64::try_from(d.as_millis()).map_err(|_| io::Error::other("system clock out of range"))
}

/// One request: an upper bound on when it reached (or will reach) Hermes, and its id (0 for
/// an entry read from the format without ids).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    at: u64,
    id: u64,
}

/// The state in the file: the time until which every request waits (after a 429), and the
/// entries of the requests that still count.
#[derive(Debug, Default, PartialEq, Eq)]
struct State {
    blocked_until: u64,
    entries: Vec<Entry>,
}

impl State {
    /// `blocked_until` on the first line, one entry per line (`<time> <id>`, or `<time>`),
    /// then `end`. `None` if malformed or incomplete (no `end` line: an empty or cut file).
    fn parse(text: &str) -> Option<Self> {
        let mut lines: Vec<&str> = text.lines().map(str::trim).collect();
        if lines.pop() != Some("end") {
            return None;
        }
        let (first, rest) = lines.split_first()?;
        let blocked_until = first.parse().ok()?;
        let entries = rest
            .iter()
            .map(|l| {
                let mut fields = l.split_whitespace();
                let at = fields.next()?.parse().ok()?;
                let id = match fields.next() {
                    Some(f) => f.parse().ok()?,
                    None => 0,
                };
                fields.next().is_none().then_some(Entry { at, id })
            })
            .collect::<Option<Vec<Entry>>>()?;
        Some(Self {
            blocked_until,
            entries,
        })
    }

    fn render(&self) -> String {
        use std::fmt::Write as _;
        let mut s = String::new();
        let _ = writeln!(s, "{}", self.blocked_until);
        for e in &self.entries {
            let _ = writeln!(s, "{} {}", e.at, e.id);
        }
        s.push_str("end\n");
        s
    }
}

/// A request admitted by [`HermesLimiter::acquire`]; pass it to [`HermesLimiter::confirm`]
/// once the request has completed, answered or failed. Dropping it unconfirmed is safe: the
/// entry keeps its latest arrival time until it expires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use = "confirm the slot once the request has completed"]
pub struct Slot {
    id: u64,
}

/// What a take found: go now, or wait this many milliseconds.
enum Take {
    Admitted,
    Wait(u64),
}

/// A host-wide sliding-window limit on Hermes requests for one API key.
#[derive(Debug, Clone)]
pub struct HermesLimiter {
    /// The state file (replaced atomically).
    path: PathBuf,
    /// The lock file (never replaced: a stable inode for `flock`).
    lock: PathBuf,
    /// The temporary file the next state is written to before the rename.
    tmp: PathBuf,
    max: usize,
    window_ms: u64,
    block_ms: u64,
    pending_ms: u64,
}

impl HermesLimiter {
    /// The limiter for `api_key` in `dir` (normally `pyth_post::default_lock_dir()`), allowing
    /// `per_minute` requests in any 60 s window.
    ///
    /// # Errors
    ///
    /// The directory cannot be created, or `per_minute` is zero.
    pub fn for_key(dir: &Path, api_key: &str, per_minute: u32) -> io::Result<Self> {
        Self::with_window(dir, api_key, per_minute, WINDOW_MS, BLOCK_MS)
    }

    /// As [`HermesLimiter::for_key`] with an explicit window and block time (tests run the
    /// same code on a scaled clock: a short window).
    ///
    /// # Errors
    ///
    /// The directory cannot be created, `max` is zero, or the window is zero.
    pub fn with_window(
        dir: &Path,
        api_key: &str,
        max: u32,
        window_ms: u64,
        block_ms: u64,
    ) -> io::Result<Self> {
        if max == 0 || window_ms == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the Hermes limit needs at least one request per non-empty window",
            ));
        }
        std::fs::create_dir_all(dir)?;
        let max = usize::try_from(max).map_err(|_| io::Error::other("limit out of range"))?;
        let base = format!("hermes-{:016x}", fnv1a(api_key.as_bytes()));
        Ok(Self {
            path: dir.join(format!("{base}.state")),
            lock: dir.join(format!("{base}.lock")),
            tmp: dir.join(format!("{base}.state.tmp")),
            max,
            window_ms,
            block_ms,
            pending_ms: PENDING_MS,
        })
    }

    /// The same limiter with another pending bound (tests run on a scaled clock).
    #[must_use]
    pub fn with_pending(mut self, pending_ms: u64) -> Self {
        self.pending_ms = pending_ms;
        self
    }

    /// The shared state file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The state under the lock: a missing file is a fresh budget; an unreadable or incomplete
    /// one is a full window and a block from `now` (it can only delay requests).
    fn read_state(&self, now: u64) -> io::Result<State> {
        let text = match File::open(&self.path) {
            Ok(mut f) => {
                let mut text = String::new();
                f.read_to_string(&mut text)?;
                text
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(State::default()),
            Err(e) => return Err(e),
        };
        Ok(State::parse(&text).unwrap_or_else(|| State {
            blocked_until: now.saturating_add(self.block_ms),
            entries: vec![Entry { at: now, id: 0 }; self.max],
        }))
    }

    /// Writes `state` to the temporary file, syncs it, and renames it over the state file.
    fn write_state(&self, state: &State) -> io::Result<()> {
        let mut tmp = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&self.tmp)?;
        tmp.write_all(state.render().as_bytes())?;
        tmp.sync_all()?;
        drop(tmp);
        std::fs::rename(&self.tmp, &self.path)
    }

    /// Runs `f` on the state under the exclusive lock and writes back what it leaves.
    fn update<T>(&self, f: impl FnOnce(&mut State, u64) -> T) -> io::Result<T> {
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&self.lock)?;
        lock.lock()?;
        let now = now_ms()?;
        let mut state = self.read_state(now)?;
        let out = f(&mut state, now);
        self.write_state(&state)?;
        // The lock is released when `lock` is dropped.
        drop(lock);
        Ok(out)
    }

    fn try_take(&self, id: u64) -> io::Result<Take> {
        let (max, window, pending) = (self.max, self.window_ms, self.pending_ms);
        self.update(|s, now| {
            let horizon = now.saturating_sub(window);
            s.entries.retain(|e| e.at > horizon);
            if now < s.blocked_until {
                return Take::Wait(s.blocked_until.saturating_sub(now));
            }
            if s.entries.len() < max {
                s.entries.push(Entry {
                    at: now.saturating_add(pending),
                    id,
                });
                return Take::Admitted;
            }
            // The earliest entry leaves the window at its time plus the window, unless it is
            // pending and confirmed sooner: look again at least every tenth of a window (at
            // most every second).
            let earliest = s.entries.iter().map(|e| e.at).min().unwrap_or(now);
            let poll = window.checked_div(10).unwrap_or(0).clamp(1, 1_000);
            Take::Wait(
                earliest
                    .saturating_add(window)
                    .saturating_sub(now)
                    .clamp(1, poll),
            )
        })
    }

    /// Admits one request, waiting as long as needed; [`HermesLimiter::confirm`] the slot once
    /// the request has completed.
    ///
    /// # Errors
    ///
    /// The file cannot be opened, locked, read or written.
    pub fn acquire(&self) -> io::Result<Slot> {
        let id = (u64::from(std::process::id()) << 32)
            | u64::from(NEXT_SLOT.fetch_add(1, Ordering::Relaxed));
        loop {
            match self.try_take(id)? {
                Take::Admitted => return Ok(Slot { id }),
                Take::Wait(ms) => std::thread::sleep(Duration::from_millis(ms)),
            }
        }
    }

    /// The request of `slot` has completed (answered or failed): it reached Hermes before now,
    /// so its entry moves to now. An entry that already left the window (a send delayed past
    /// the pending bound) is recorded again at now.
    ///
    /// # Errors
    ///
    /// The file cannot be opened, locked, read or written.
    pub fn confirm(&self, slot: Slot) -> io::Result<()> {
        self.update(
            |s, now| match s.entries.iter_mut().find(|e| e.id == slot.id) {
                Some(e) => e.at = now,
                None => s.entries.push(Entry {
                    at: now,
                    id: slot.id,
                }),
            },
        )
    }

    /// After a 429: no process on this file sends before the block time has passed.
    ///
    /// # Errors
    ///
    /// The file cannot be opened, locked, read or written.
    pub fn block(&self) -> io::Result<()> {
        let block = self.block_ms;
        self.update(|s, now| {
            s.blocked_until = s.blocked_until.max(now.saturating_add(block));
        })
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_macros, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_state_roundtrip_and_malformed() {
        let s = State {
            blocked_until: 7,
            entries: vec![Entry { at: 1, id: 9 }, Entry { at: 2, id: 10 }],
        };
        assert_eq!(State::parse(&s.render()), Some(s));
        // The format without ids: confirmed entries with id 0.
        assert_eq!(
            State::parse("7\n1\n2\nend\n"),
            Some(State {
                blocked_until: 7,
                entries: vec![Entry { at: 1, id: 0 }, Entry { at: 2, id: 0 }],
            })
        );
        assert_eq!(State::parse("7\n1 2 3\nend\n"), None);
        // Empty, cut before `end`, or malformed: never a fresh budget.
        assert_eq!(State::parse(""), None);
        assert_eq!(State::parse("0\n"), None);
        assert_eq!(State::parse("7\n1\n2\n"), None);
        assert_eq!(State::parse("12\nnot a number\nend\n"), None);
        assert_eq!(State::parse("0\nend\n"), Some(State::default()));
    }

    #[test]
    fn test_fnv_is_stable_and_names_keys_apart() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_ne!(fnv1a(b"key-1"), fnv1a(b"key-2"));
    }
}
