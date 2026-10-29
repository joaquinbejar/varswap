//! The boundary to the log-return math, filled by optfix `ln` in milestone 3.

use optfix::{Fixed, PricingError};

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
/// kernel the program uses until milestone 1b plugs in optfix `ln`. Only with the
/// `insecure-test-kernel` feature; a program built with it must not be deployed.
#[cfg(feature = "insecure-test-kernel")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IntegerKernel;

#[cfg(feature = "insecure-test-kernel")]
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

#[cfg(feature = "insecure-test-kernel")]
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

/// `ln 10` in Q64.64, rounded to nearest (`round(ln 10 × 2^64)`, from a 80-digit decimal
/// computation; within half an ulp).
pub const LN_10_RAW: i128 = 42_475_197_918_399_869_020;

/// The production kernel: optfix `ln` (Q64.64, within [`optfix::LN_MAX_ERROR_ULPS`] = 2 ulp
/// of `2^-64`, rounded to nearest).
///
/// `r = ln(to) − ln(from) + (e_to − e_from) × ln 10`, with `ln` applied to each price
/// mantissa (at most `i64::MAX`, inside Q64.64). The ratio is never formed, so prints with
/// very different exponents cannot overflow. `r²` is rounded down (`mul_down`) and
/// [`LnKernel::annualize`] rounds down: toward a lower realized variance, the short's side,
/// matching the payout rounding of `docs/PLAN.md` 4.4.
///
/// Error of one term (raw units, `2^-64`), with `δr` the error of `r`:
/// `δr <= 2 × LN_MAX_ERROR_ULPS + |Δe|` (two `ln` calls, plus [`LN_10_RAW`]'s half ulp
/// scaled by `|Δe|`, rounded up to one), and
/// `|computed r² − r²| <= 2 |r| δr + δr² + 1` ([`LnKernel::term_error_bound`]). For
/// `|r| <= 1` and equal exponents that is at most 9 raw units (`4.9 × 10^-19`) per term.
/// The `ln` part is undirected; only the final `+ 1` truncation is directed (down).
/// Cost: two `ln` calls (about 900 CU) per term on SBF.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LnKernel;

impl LnKernel {
    /// `ln(to / from)` as above.
    ///
    /// # Errors
    ///
    /// [`CoreError::Kernel`] with the optfix `PricingError` code, or
    /// [`CoreError::Overflow`].
    pub fn log_return(from: StoredPrice, to: StoredPrice) -> Result<Fixed, CoreError> {
        let a = ln_mantissa(to)?;
        let b = ln_mantissa(from)?;
        let mut r = a.checked_sub(b).map_err(kernel_err)?;
        let de = i128::from(to.exponent())
            .checked_sub(i128::from(from.exponent()))
            .ok_or(CoreError::Overflow)?;
        if de != 0 {
            // An integer times a Q64.64 raw value is exact.
            let shift = LN_10_RAW.checked_mul(de).ok_or(CoreError::Overflow)?;
            r = r.checked_add(Fixed::from_raw(shift)).map_err(kernel_err)?;
        }
        Ok(r)
    }

    /// Upper bound, in raw units, on the error of [`ReturnKernel::squared_log_return`] for a
    /// term whose log return is `r` and whose exponents differ by `delta_exp`.
    ///
    /// # Errors
    ///
    /// [`CoreError::Overflow`].
    pub fn term_error_bound(r: Fixed, delta_exp: u8) -> Result<u128, CoreError> {
        let ln_err = u128::from(optfix::LN_MAX_ERROR_ULPS);
        let dr = ln_err
            .checked_mul(2)
            .and_then(|x| x.checked_add(u128::from(delta_exp)))
            .ok_or(CoreError::Overflow)?;
        let r_abs = r.raw().unsigned_abs();
        // 2 |r| δr in raw units: (2 × r_raw × δr) / 2^64, rounded up (floor + 1); then + 1
        // for δr² / 2^64 (at most 1 for δr below 2^32) and + 1 for the `mul_down` truncation.
        let two_r_dr = r_abs
            .checked_mul(dr)
            .and_then(|x| x.checked_mul(2))
            .ok_or(CoreError::Overflow)?;
        let scaled = two_r_dr
            .checked_shr(64)
            .and_then(|x| x.checked_add(1))
            .ok_or(CoreError::Overflow)?;
        scaled.checked_add(2).ok_or(CoreError::Overflow)
    }
}

impl LnKernel {
    /// Upper bound, in Q64.64 raw units, on `|computed − exact|` realized variance of a
    /// complete window on a grid of `interval` seconds, for payoffs that use
    /// `min(realized, cap_var)` (`cap_var` in Q64.64 raw units) and for exponent changes of at
    /// most `max_delta_exp` between adjacent buckets:
    ///
    /// `ε <= 2 δr √(cap_var × Y) + 3 Y + 2`, `Y = ceil(YEAR_SECONDS / interval)`,
    /// `δr = 2 × LN_MAX_ERROR_ULPS + max_delta_exp`.
    ///
    /// Derivation: each term is within `2 |r_i| δr + 3` raw units
    /// ([`LnKernel::term_error_bound`]); summed over `N` terms and annualized by `F =
    /// YEAR_SECONDS / (N × interval)`, the error is `F (2 δr Σ|r_i| + 3N) + 1`. By
    /// Cauchy–Schwarz `Σ|r_i| <= √(N Σ r_i²)`, and `F Σ r_i² = V`, the realized variance, so
    /// `F × 2 δr √(N Σ r_i²) = 2 δr √(V × F × N) = 2 δr √(V × Y)` and `F × 3N = 3Y`: the bound
    /// does not depend on `N`. The payoff only reads `min(V, cap_var)`, which is 1-Lipschitz,
    /// and `V − ε(V)` increases with `V`, so evaluating at `V = cap_var` covers every path.
    ///
    /// # Errors
    ///
    /// [`CoreError::ZeroInterval`] or [`CoreError::Overflow`].
    pub fn realized_var_error_bound(
        interval: crate::Seconds,
        cap_var: u128,
        max_delta_exp: u8,
    ) -> Result<u128, CoreError> {
        let iv = u128::from(interval.get());
        if iv == 0 {
            return Err(CoreError::ZeroInterval);
        }
        let year = u128::from(crate::YEAR_SECONDS);
        let y = year
            .checked_add(iv)
            .and_then(|x| x.checked_sub(1))
            .and_then(|x| x.checked_div(iv))
            .ok_or(CoreError::Overflow)?;
        let dr = u128::from(optfix::LN_MAX_ERROR_ULPS)
            .checked_mul(2)
            .and_then(|x| x.checked_add(u128::from(max_delta_exp)))
            .ok_or(CoreError::Overflow)?;
        // V = cap_var / 2^64 (real), so √(V × Y) = √(cap_var × Y) / 2^32. δr counts raw
        // units, hence 2 δr √(V Y) raw units = 2 δr √(cap_var × Y) / 2^32, rounded up
        // (isqrt + 1, then shift + 1).
        let q = cap_var.checked_mul(y).ok_or(CoreError::Overflow)?;
        let root = q.isqrt().checked_add(1).ok_or(CoreError::Overflow)?;
        let main_raw = root
            .checked_mul(dr)
            .and_then(|x| x.checked_mul(2))
            .and_then(|x| x.checked_shr(32))
            .and_then(|x| x.checked_add(1))
            .ok_or(CoreError::Overflow)?;
        main_raw
            .checked_add(y.checked_mul(3).ok_or(CoreError::Overflow)?)
            .and_then(|x| x.checked_add(2))
            .ok_or(CoreError::Overflow)
    }
}

fn ln_mantissa(p: StoredPrice) -> Result<Fixed, CoreError> {
    optfix::ln(Fixed::from_int(p.price())).map_err(kernel_err)
}

#[cold]
fn kernel_err(e: PricingError) -> CoreError {
    CoreError::Kernel(e as u8)
}

impl ReturnKernel for LnKernel {
    fn squared_log_return(&self, from: StoredPrice, to: StoredPrice) -> Result<SumSq, CoreError> {
        let r = Self::log_return(from, to)?;
        let sq = r.mul_down(r).map_err(kernel_err)?;
        u128::try_from(sq.raw())
            .map(SumSq::from_raw)
            .map_err(|_| CoreError::Overflow)
    }

    /// `sum_sq × YEAR_SECONDS / span` in Q64.64 raw units, rounded down (toward the short).
    fn annualize(&self, sum_sq: SumSq, span_seconds: u64) -> Result<u128, CoreError> {
        sum_sq
            .raw()
            .checked_mul(u128::from(crate::YEAR_SECONDS))
            .and_then(|x| x.checked_div(u128::from(span_seconds)))
            .ok_or(CoreError::Overflow)
    }
}
