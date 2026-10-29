//! The boundary to the log-return math, filled by optfix `ln` in milestone 3.

use crate::error::CoreError;
use crate::print::StoredPrice;

/// Sum of squared log returns, in the raw fixed-point units of the [`ReturnKernel`] that
/// produced it. Never negative.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub struct SumSq(u128);

impl SumSq {
    /// Zero: no pair of adjacent buckets filled yet.
    pub const ZERO: Self = Self(0);

    /// Wraps a raw value.
    #[inline]
    #[must_use]
    pub const fn from_raw(raw: u128) -> Self {
        Self(raw)
    }

    /// The raw value.
    #[inline]
    #[must_use]
    pub const fn raw(self) -> u128 {
        self.0
    }

    /// `self + term`.
    ///
    /// # Errors
    ///
    /// [`CoreError::Overflow`].
    #[inline]
    pub fn checked_add(self, term: Self) -> Result<Self, CoreError> {
        self.0
            .checked_add(term.0)
            .map(Self)
            .ok_or(CoreError::Overflow)
    }
}

/// Squared log returns and annualization. The window only adds terms and calls
/// [`ReturnKernel::annualize`] once; the kernel owns the units and every rounding.
pub trait ReturnKernel {
    /// `ln(to / from)²`, each price `price × 10^exponent` with its own exponent (log returns
    /// are scale invariant), in the kernel's raw units.
    ///
    /// # Errors
    ///
    /// [`CoreError::Kernel`] or [`CoreError::Overflow`].
    fn squared_log_return(&self, from: StoredPrice, to: StoredPrice) -> Result<SumSq, CoreError>;

    /// Annualized variance `sum_sq × YEAR_SECONDS / span_seconds` in the kernel's raw units,
    /// with its documented rounding.
    ///
    /// # Errors
    ///
    /// [`CoreError::Kernel`] or [`CoreError::Overflow`].
    fn annualize(&self, sum_sq: SumSq, span_seconds: u64) -> Result<u128, CoreError>;
}
