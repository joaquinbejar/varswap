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

/// Exact integer stand-in for the log-return kernel: `(to − from)²` with both prices
/// rescaled to the smaller exponent, and `sum_sq × YEAR_SECONDS / span` rounded down.
///
/// **Not a log return.** It is exact, so tests compare windows bit for bit, and it is the
/// kernel the program uses until milestone 1b plugs in optfix `ln`; a program built with it
/// must not be deployed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IntegerKernel;

impl IntegerKernel {
    /// `price × 10^(exponent − base)`, `exponent >= base`.
    fn rescale(p: StoredPrice, base: i8) -> Result<i128, CoreError> {
        let shift = i32::from(p.exponent())
            .checked_sub(i32::from(base))
            .and_then(|s| u32::try_from(s).ok())
            .ok_or(CoreError::Overflow)?;
        10i128
            .checked_pow(shift)
            .and_then(|f| f.checked_mul(i128::from(p.price())))
            .ok_or(CoreError::Overflow)
    }
}

impl ReturnKernel for IntegerKernel {
    fn squared_log_return(&self, from: StoredPrice, to: StoredPrice) -> Result<SumSq, CoreError> {
        let base = from.exponent().min(to.exponent());
        let d = Self::rescale(to, base)?
            .checked_sub(Self::rescale(from, base)?)
            .ok_or(CoreError::Overflow)?;
        d.checked_mul(d)
            .and_then(|sq| u128::try_from(sq).ok())
            .map(SumSq::from_raw)
            .ok_or(CoreError::Overflow)
    }

    fn annualize(&self, sum_sq: SumSq, span_seconds: u64) -> Result<u128, CoreError> {
        // Rounded down; the real kernel documents its own direction.
        sum_sq
            .raw()
            .checked_mul(u128::from(crate::YEAR_SECONDS))
            .and_then(|x| x.checked_div(u128::from(span_seconds)))
            .ok_or(CoreError::Overflow)
    }
}
