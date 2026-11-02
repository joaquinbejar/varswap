//! One Hermes request budget for every varswap process on a host (`docs/PLAN-improvements.md`,
//! I6).
//!
//! The Hermes API key is shared with metalbook; varswap's allocation is `R` requests per minute
//! across all its processes (`docs/governance.md`: 3). [`HermesLimiter`] enforces it where
//! requests leave the process: before every HTTP request (retries and requests answered 404 or
//! 429 included) a process takes a slot from a file shared by every process using the same lock
//! directory, under an exclusive `flock`. The file holds the send times of the last requests;
//! a request may leave only if fewer than `R` of them fall in the last window (60 s), so at
//! most `R` requests leave in any sliding window. A 429 blocks every process on that file for
//! the block time (61 s: Hermes asks for 60): no request takes a slot until it has passed.
//! Requests that took their slot before the 429 was recorded (at most `R − 1` others, from the
//! same window) are already in flight and still arrive.
//!
//! The file is named after a hash of the API key (64-bit FNV-1a, never the key itself), so two
//! keys never share a budget. A file that cannot be parsed (a crash mid-write) is read as a full
//! window, which only delays requests. The scope is one host: processes on another machine do
//! not see this file.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// varswap's share of the shared key's quota on the timestamp endpoint, per minute.
pub const DEFAULT_REQUESTS_PER_MINUTE: u32 = 3;
/// The sliding window, in milliseconds.
pub const WINDOW_MS: u64 = 60_000;
/// How long a 429 blocks every process, in milliseconds (Hermes asks for 60 s).
pub const BLOCK_MS: u64 = 61_000;

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

/// The state in the file: the time until which every request waits (after a 429), and the
/// send times of the requests in the current window.
#[derive(Debug, Default, PartialEq, Eq)]
struct State {
    blocked_until: u64,
    sends: Vec<u64>,
}

impl State {
    /// `blocked_until` on the first line, then one send time per line. `None` if malformed.
    fn parse(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        let Some(first) = lines.next() else {
            return Some(Self::default());
        };
        let blocked_until = first.trim().parse().ok()?;
        let sends = lines
            .map(|l| l.trim().parse().ok())
            .collect::<Option<Vec<u64>>>()?;
        Some(Self {
            blocked_until,
            sends,
        })
    }

    fn render(&self) -> String {
        use std::fmt::Write as _;
        let mut s = String::new();
        let _ = writeln!(s, "{}", self.blocked_until);
        for t in &self.sends {
            let _ = writeln!(s, "{t}");
        }
        s
    }
}

/// What a take found: go now, or wait this many milliseconds.
enum Take {
    Sent,
    Wait(u64),
}

/// A host-wide sliding-window limit on Hermes requests for one API key.
#[derive(Debug, Clone)]
pub struct HermesLimiter {
    path: PathBuf,
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
        Ok(Self {
            path: dir.join(format!("hermes-{:016x}.limit", fnv1a(api_key.as_bytes()))),
            max,
            window_ms,
            block_ms,
        })
    }

    /// The shared file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn open(&self) -> io::Result<File> {
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&self.path)
    }

    /// Runs `f` on the state under the exclusive lock and writes back what it returns.
    fn update<T>(&self, f: impl FnOnce(&mut State, u64) -> T) -> io::Result<T> {
        let mut file = self.open()?;
        file.lock()?;
        let mut text = String::new();
        file.read_to_string(&mut text)?;
        let now = now_ms()?;
        // Unreadable state counts as a full window from now: it can only delay requests.
        let mut state = State::parse(&text).unwrap_or_else(|| State {
            blocked_until: 0,
            sends: vec![now; self.max],
        });
        let out = f(&mut state, now);
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(state.render().as_bytes())?;
        file.flush()?;
        // The lock is released when `file` is dropped.
        Ok(out)
    }

    fn try_take(&self) -> io::Result<Take> {
        let (max, window) = (self.max, self.window_ms);
        self.update(|s, now| {
            let horizon = now.saturating_sub(window);
            s.sends.retain(|t| *t > horizon);
            if now < s.blocked_until {
                return Take::Wait(s.blocked_until.saturating_sub(now));
            }
            if s.sends.len() < max {
                s.sends.push(now);
                return Take::Sent;
            }
            // The oldest send in the window leaves it at oldest + window.
            let oldest = s.sends.iter().copied().min().unwrap_or(now);
            Take::Wait(oldest.saturating_add(window).saturating_sub(now).max(1))
        })
    }

    /// Takes one request slot, waiting as long as needed.
    ///
    /// # Errors
    ///
    /// The file cannot be opened, locked, read or written.
    pub fn acquire(&self) -> io::Result<()> {
        loop {
            match self.try_take()? {
                Take::Sent => return Ok(()),
                Take::Wait(ms) => std::thread::sleep(Duration::from_millis(ms)),
            }
        }
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
            sends: vec![1, 2, 3],
        };
        assert_eq!(State::parse(&s.render()), Some(s));
        assert_eq!(State::parse(""), Some(State::default()));
        assert_eq!(State::parse("12\nnot a number\n"), None);
    }

    #[test]
    fn test_fnv_is_stable_and_names_keys_apart() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_ne!(fnv1a(b"key-1"), fnv1a(b"key-2"));
    }
}
