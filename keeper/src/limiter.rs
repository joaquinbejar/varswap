//! One Hermes request budget for every varswap process on a host (`docs/PLAN-improvements.md`,
//! I6).
//!
//! The Hermes API key is shared with metalbook; varswap's allocation is `R` requests per minute
//! across all its processes (`docs/governance.md`: 3). The contract is wire time: Hermes counts
//! the requests that reach it, so at most `R` requests may arrive in any sliding window (60 s),
//! retries and requests answered 404 or 429 included, whatever happens to a process between
//! deciding to send and sending. [`HermesLimiter`] enforces it with one file of entries shared
//! by every process using the same lock directory, under an exclusive `flock`, one entry per
//! request:
//!
//! - [`HermesLimiter::acquire`] admits a request only if fewer than `R` entries still count,
//!   and records it as pending. The admitting process creates and locks (`flock`) a file of
//!   its own for that request and holds it in the returned [`Slot`] until the request is over.
//! - A pending entry counts for as long as that lock is held: however long the process takes
//!   to send, stopped (SIGSTOP) or not. When another process finds the lock free (the owner
//!   died or dropped the slot), the request is over too, and the entry becomes a completed one
//!   at the time it was found.
//! - [`HermesLimiter::confirm`], called once the request has been answered or has failed,
//!   marks the entry completed at that time: the request reached Hermes before then, if at all.
//! - A completed entry counts until its completion time plus the window.
//!
//! So every entry counts from before its request can be sent until a window after it can have
//! arrived, and no `R + 1` requests arrive within one window. A process stopped in the middle
//! of a request keeps holding one of the `R` slots until it resumes or dies; the others share
//! the rest.
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
//! `<time> <request id>` for completed requests and `<time> <request id> pending`; a line with
//! a time only (the earlier format) is a completed entry, while earlier binaries read the new
//! format conservatively, so every varswap process on a host is upgraded together. The scope is
//! one host: processes on another machine do not see these files.

use std::fs::{File, OpenOptions, TryLockError};
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

/// One request: its id (0 for an entry read from the format without ids) and, once completed,
/// a time by which it had reached Hermes if it ever did; while `pending`, `at` is the admission
/// time and the entry counts for as long as its owner holds the request's lock file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    at: u64,
    id: u64,
    pending: bool,
}

/// The state in the file: the time until which every request waits (after a 429), and the
/// entries of the requests that still count.
#[derive(Debug, Default, PartialEq, Eq)]
struct State {
    blocked_until: u64,
    entries: Vec<Entry>,
}

impl State {
    /// `blocked_until` on the first line, one entry per line (`<time> <id>`,
    /// `<time> <id> pending`, or `<time>`), then `end`. `None` if malformed or incomplete (no `end` line: an empty or cut file).
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
                let pending = match fields.next() {
                    Some("pending") => true,
                    Some(_) => return None,
                    None => false,
                };
                fields.next().is_none().then_some(Entry { at, id, pending })
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
            let _ = if e.pending {
                writeln!(s, "{} {} pending", e.at, e.id)
            } else {
                writeln!(s, "{} {}", e.at, e.id)
            };
        }
        s.push_str("end\n");
        s
    }
}

/// A request admitted by [`HermesLimiter::acquire`], holding the lock that keeps its entry
/// pending; pass it to [`HermesLimiter::confirm`] once the request has been answered or has
/// failed. Dropping it unconfirmed releases the lock: the next process to look records the
/// request as completed at that time.
#[derive(Debug)]
#[must_use = "confirm the slot once the request has completed"]
pub struct Slot {
    id: u64,
    path: PathBuf,
    /// The locked request file; the lock lasts as long as this handle.
    _guard: File,
}

/// The lock file of request `id` of the limiter whose files start with `base` in `dir`.
fn request_file(dir: &Path, base: &str, id: u64) -> PathBuf {
    dir.join(format!("{base}.{id:016x}.request"))
}

/// Whether the owner of a pending request still holds its lock file at `path`. A file that is
/// missing or can be locked has no owner: the request is over (with `reap`, the file is then
/// removed). Anything else (an I/O error) is read as held, which only delays requests.
fn owner_holds(path: &Path, reap: bool) -> bool {
    let file = match OpenOptions::new().write(true).open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return false,
        Err(_) => return true,
    };
    match file.try_lock() {
        Ok(()) => {
            drop(file);
            if reap {
                let _ = std::fs::remove_file(path);
            }
            false
        }
        Err(TryLockError::WouldBlock) => true,
        Err(TryLockError::Error(_)) => true,
    }
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
    /// The directory and file name prefix of the per-request lock files.
    dir: PathBuf,
    base: String,
    max: usize,
    window_ms: u64,
    block_ms: u64,
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
            dir: dir.to_path_buf(),
            base,
            max,
            window_ms,
            block_ms,
        })
    }

    /// The lock file of request `id`.
    fn request_path(&self, id: u64) -> PathBuf {
        request_file(&self.dir, &self.base, id)
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
            entries: vec![
                Entry {
                    at: now,
                    id: 0,
                    pending: false,
                };
                self.max
            ],
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

    /// Admits request `id` if the budget allows, creating and locking its request file first;
    /// otherwise says how long to wait before looking again.
    fn try_take(&self, id: u64) -> io::Result<Result<Slot, u64>> {
        let (max, window) = (self.max, self.window_ms);
        self.update(|s, now| {
            // A pending request whose owner no longer holds its lock is over: it reached
            // Hermes, if at all, before now.
            for e in s.entries.iter_mut().filter(|e| e.pending) {
                if !owner_holds(&self.request_path(e.id), true) {
                    e.pending = false;
                    e.at = now;
                }
            }
            let horizon = now.saturating_sub(window);
            s.entries.retain(|e| e.pending || e.at > horizon);
            if now < s.blocked_until {
                return Ok(Err(s.blocked_until.saturating_sub(now)));
            }
            if s.entries.len() < max {
                let path = self.request_path(id);
                let guard = OpenOptions::new()
                    .create(true)
                    .truncate(false)
                    .write(true)
                    .open(&path)?;
                guard.lock()?;
                s.entries.push(Entry {
                    at: now,
                    id,
                    pending: true,
                });
                return Ok(Ok(Slot {
                    id,
                    path,
                    _guard: guard,
                }));
            }
            // The earliest completed entry leaves the window at its time plus the window; a
            // pending one may complete at any time: look again at least every tenth of a
            // window (at most every second).
            let poll = window.checked_div(10).unwrap_or(0).clamp(1, 1_000);
            let wait = s
                .entries
                .iter()
                .filter(|e| !e.pending)
                .map(|e| e.at.saturating_add(window).saturating_sub(now))
                .min()
                .unwrap_or(poll);
            Ok(Err(wait.clamp(1, poll)))
        })?
    }

    /// Admits one request, waiting as long as needed; [`HermesLimiter::confirm`] the slot once
    /// the request has been answered or has failed.
    ///
    /// # Errors
    ///
    /// A file cannot be opened, locked, read or written.
    pub fn acquire(&self) -> io::Result<Slot> {
        let id = (u64::from(std::process::id()) << 32)
            | u64::from(NEXT_SLOT.fetch_add(1, Ordering::Relaxed));
        loop {
            match self.try_take(id)? {
                Ok(slot) => return Ok(slot),
                Err(ms) => std::thread::sleep(Duration::from_millis(ms)),
            }
        }
    }

    /// The request of `slot` has been answered or has failed: it reached Hermes before now, if
    /// at all, so its entry is completed at now and counts for one more window. Then the
    /// request's lock is released.
    ///
    /// # Errors
    ///
    /// The file cannot be opened, locked, read or written (the lock is released all the same:
    /// the next process to look records the request as completed then).
    pub fn confirm(&self, slot: Slot) -> io::Result<()> {
        let id = slot.id;
        let out = self.update(|s, now| match s.entries.iter_mut().find(|e| e.id == id) {
            Some(e) => {
                e.pending = false;
                e.at = now;
            }
            None => s.entries.push(Entry {
                at: now,
                id,
                pending: false,
            }),
        });
        let _ = std::fs::remove_file(&slot.path);
        drop(slot);
        out
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

/// A request in progress: admitted, not yet answered or failed, and its owner still holding
/// its lock (alive, possibly stopped or hung).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeldRequest {
    /// The owner's process id.
    pub pid: u32,
    /// When it was admitted, in Unix milliseconds.
    pub since_ms: u64,
}

/// The limiter state files in `dir`, one per API key (`hermes-<hash>.state`), sorted.
///
/// # Errors
///
/// The directory cannot be read.
pub fn state_files(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with("hermes-") && name.ends_with(".state") {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

/// The requests of the limiter whose state file is `state` that are still held by their owner.
/// Read only: nothing is reaped or written, and the state's own lock is not taken (the file is
/// replaced atomically, so a read sees one whole state).
///
/// # Errors
///
/// The file cannot be read or parsed, or its name is not a limiter state file's.
pub fn held_requests(state: &Path) -> io::Result<Vec<HeldRequest>> {
    let base = state
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_suffix(".state"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not a limiter state file"))?;
    let dir = state.parent().unwrap_or_else(|| Path::new("."));
    let text = std::fs::read_to_string(state)?;
    let parsed = State::parse(&text)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "unreadable limiter state"))?;
    Ok(parsed
        .entries
        .iter()
        .filter(|e| e.pending && owner_holds(&request_file(dir, base, e.id), false))
        .map(|e| HeldRequest {
            pid: u32::try_from(e.id >> 32).unwrap_or(0),
            since_ms: e.at,
        })
        .collect())
}

#[cfg(test)]
#[allow(clippy::disallowed_macros, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_state_roundtrip_and_malformed() {
        let s = State {
            blocked_until: 7,
            entries: vec![
                Entry {
                    at: 1,
                    id: 9,
                    pending: false,
                },
                Entry {
                    at: 2,
                    id: 10,
                    pending: true,
                },
            ],
        };
        assert_eq!(State::parse(&s.render()), Some(s));
        // The format without ids: confirmed entries with id 0.
        assert_eq!(
            State::parse("7\n1\n2\nend\n"),
            Some(State {
                blocked_until: 7,
                entries: vec![
                    Entry {
                        at: 1,
                        id: 0,
                        pending: false,
                    },
                    Entry {
                        at: 2,
                        id: 0,
                        pending: false,
                    },
                ],
            })
        );
        assert_eq!(State::parse("7\n1 2 3\nend\n"), None);
        assert_eq!(State::parse("7\n1 2 pending x\nend\n"), None);
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
