//! Quantity newtypes and the window state.

use crate::error::CoreError;

/// Seconds in a year used for annualization, as optfix `Years` (`docs/PLAN.md` 4.5).
pub const YEAR_SECONDS: u64 = 31_536_000;

/// A Solana account address or program id.
pub type Address = [u8; 32];

/// A non-negative duration in seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Seconds(u32);

impl Seconds {
    /// Wraps a duration in seconds.
    #[inline]
    #[must_use]
    pub const fn new(seconds: u32) -> Self {
        Self(seconds)
    }

    /// The duration in seconds.
    #[inline]
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Basis points: 1 bps = 1 / 10,000.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Bps(u16);

impl Bps {
    /// Wraps a value in basis points.
    #[inline]
    #[must_use]
    pub const fn new(bps: u16) -> Self {
        Self(bps)
    }

    /// The value in basis points.
    #[inline]
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

/// Life cycle of a window (`docs/PLAN.md` 2). Stored as a `u8` and validated on load.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum WindowState {
    /// Created, no bucket filled yet.
    Pending = 0,
    /// At least one bucket filled, not final.
    Sampling = 1,
    /// Every bucket filled; `realized_var` is set and swaps settle on it.
    Finalized = 2,
    /// A bucket was still empty at `end + backfill_deadline`; swaps unwind.
    Degraded = 3,
}

impl WindowState {
    /// `Finalized` or `Degraded`: no more samples, swaps can settle.
    #[inline]
    #[must_use]
    pub const fn is_final(self) -> bool {
        matches!(self, Self::Finalized | Self::Degraded)
    }
}

impl TryFrom<u8> for WindowState {
    type Error = CoreError;

    #[inline]
    fn try_from(v: u8) -> Result<Self, CoreError> {
        match v {
            0 => Ok(Self::Pending),
            1 => Ok(Self::Sampling),
            2 => Ok(Self::Finalized),
            3 => Ok(Self::Degraded),
            _ => Err(CoreError::InvalidAccountData),
        }
    }
}
