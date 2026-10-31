//! One poster process per payer, enforced with an exclusive lock on a file named after the
//! payer.
//!
//! [`crate::LeftoverScanner`] and every orphan sweep built on it assume the payer is exclusive
//! to one poster process: closing a leftover cannot be atomic against another process's
//! verify, so two processes posting with the same payer can break each other's posts. Take a
//! [`PayerLock`] at start and keep it for the life of the process.

use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

use solana_address::Address;

/// Environment variable that overrides the lock directory.
pub const LOCK_DIR_VAR: &str = "PYTH_POST_LOCK_DIR";

/// The lock directory: [`LOCK_DIR_VAR`], else `$HOME/.local/state/pyth-post`. A fixed path, not
/// the temporary directory, which differs between a shell and a launchd agent of the same user.
///
/// # Errors
///
/// `NotFound` if neither variable is set.
pub fn default_lock_dir() -> io::Result<PathBuf> {
    if let Some(dir) = std::env::var_os(LOCK_DIR_VAR) {
        return Ok(PathBuf::from(dir));
    }
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".local/state/pyth-post"))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "neither PYTH_POST_LOCK_DIR nor HOME is set",
            )
        })
}

/// An exclusive lock on `payer-<address>.lock`, held until it is dropped (or the process
/// exits).
#[derive(Debug)]
pub struct PayerLock {
    _file: File,
    path: PathBuf,
}

impl PayerLock {
    /// Locks `payer` in [`default_lock_dir`].
    ///
    /// # Errors
    ///
    /// As [`PayerLock::acquire_in`].
    pub fn acquire(payer: &Address) -> io::Result<Self> {
        Self::acquire_in(&default_lock_dir()?, payer)
    }

    /// Locks `payer` in `dir` (created if missing).
    ///
    /// # Errors
    ///
    /// `WouldBlock` if another process holds the lock, or the I/O error of creating the
    /// directory or the file.
    pub fn acquire_in(dir: &Path, payer: &Address) -> io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(format!("payer-{payer}.lock"));
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file, path }),
            Err(TryLockError::WouldBlock) => Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                format!(
                    "another process posts with payer {payer} (lock {}): one poster per payer",
                    path.display()
                ),
            )),
            Err(TryLockError::Error(e)) => Err(e),
        }
    }

    /// The lock file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}
