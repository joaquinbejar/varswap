//! Capped, fully collateralized variance swap terms and settlement (`docs/PLAN.md` 2, 4.4).
//!
//! Units: `notional` is atoms of the collateral mint per 1.0 of annualized variance;
//! `strike_var`, `cap_var` and realized variances are Q64.64 raw units (`raw / 2^64`).
//! Every rounding goes to the short: collateral rounds up, the long's gain rounds down and
//! its loss rounds up, and the payout reads the conservative variance
//! ([`crate::LnKernel::conservative_realized_var`]).

use crate::error::CoreError;
use crate::kernel::LnKernel;
use crate::types::Seconds;

/// Largest cap: 2^20 = 1,048,576 in variance units (about 102,400% vol). Keeps every
/// product below 2^128 and the error bound's arithmetic in range.
pub const MAX_CAP_VAR: u128 = 1 << (64 + 20);

/// Validated swap terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwapTerms {
    notional: u64,
    strike_var: u128,
    cap_var: u128,
    long_collateral: u64,
    short_collateral: u64,
}

/// How a swap settles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settlement {
    /// The window finalized; the payout reads this conservative realized variance (raw).
    Realized(u128),
    /// The window degraded: each side gets its own collateral back.
    Unwind,
}

/// `floor(n × x / 2^64)` or the ceiling, exact, for `n < 2^64` and any `x`: `x = hi × 2^64 +
/// lo` gives `n × hi + n × lo / 2^64`, each product below `2^128`.
fn mul_shr64(n: u64, x: u128, round_up: bool) -> Result<u128, CoreError> {
    let n = u128::from(n);
    let hi = x.checked_shr(64).ok_or(CoreError::Overflow)?;
    let lo = x & u128::from(u64::MAX);
    let lo_prod = n.checked_mul(lo).ok_or(CoreError::Overflow)?;
    let mut q = n
        .checked_mul(hi)
        .and_then(|a| a.checked_add(lo_prod.checked_shr(64)?))
        .ok_or(CoreError::Overflow)?;
    if round_up && lo_prod & u128::from(u64::MAX) != 0 {
        q = q.checked_add(1).ok_or(CoreError::Overflow)?;
    }
    Ok(q)
}

fn to_atoms(x: u128) -> Result<u64, CoreError> {
    u64::try_from(x).map_err(|_| CoreError::CollateralOverflow)
}

impl SwapTerms {
    /// Validates terms on a window of grid `interval` and computes the collateral:
    /// `long = ceil(notional × strike)`, `short = ceil(notional × (cap − strike))`, both
    /// rounded up (toward the short's protection and the long's, so the vault always covers
    /// the payout), and `long + short` must fit in a `u64`.
    ///
    /// Rejects `notional × ε(interval, cap) >= 2^64` (the realized variance error bound of
    /// `docs/PLAN.md` 4.4, exponent pinned), so the short's rounding advantage stays under
    /// three atoms.
    ///
    /// # Errors
    ///
    /// [`CoreError::ZeroNotional`], [`CoreError::InvalidStrike`], [`CoreError::InvalidCap`],
    /// [`CoreError::NotionalAboveErrorBound`], [`CoreError::CollateralOverflow`].
    pub fn try_new(
        notional: u64,
        strike_var: u128,
        cap_var: u128,
        interval: Seconds,
    ) -> Result<Self, CoreError> {
        if notional == 0 {
            return Err(CoreError::ZeroNotional);
        }
        if strike_var == 0 {
            return Err(CoreError::InvalidStrike);
        }
        if cap_var <= strike_var || cap_var > MAX_CAP_VAR {
            return Err(CoreError::InvalidCap);
        }
        let eps = LnKernel::realized_var_error_bound(interval, cap_var, 0)?;
        let err = u128::from(notional)
            .checked_mul(eps)
            .ok_or(CoreError::NotionalAboveErrorBound)?;
        if err >= 1 << 64 {
            return Err(CoreError::NotionalAboveErrorBound);
        }
        let long_collateral = to_atoms(mul_shr64(notional, strike_var, true)?)?;
        let span = cap_var.checked_sub(strike_var).ok_or(CoreError::Overflow)?;
        let short_collateral = to_atoms(mul_shr64(notional, span, true)?)?;
        long_collateral
            .checked_add(short_collateral)
            .ok_or(CoreError::CollateralOverflow)?;
        Ok(Self {
            notional,
            strike_var,
            cap_var,
            long_collateral,
            short_collateral,
        })
    }

    /// Atoms per unit of variance.
    #[inline]
    #[must_use]
    pub const fn notional(&self) -> u64 {
        self.notional
    }

    /// Strike variance, raw.
    #[inline]
    #[must_use]
    pub const fn strike_var(&self) -> u128 {
        self.strike_var
    }

    /// Cap variance, raw.
    #[inline]
    #[must_use]
    pub const fn cap_var(&self) -> u128 {
        self.cap_var
    }

    /// The long's collateral, atoms.
    #[inline]
    #[must_use]
    pub const fn long_collateral(&self) -> u64 {
        self.long_collateral
    }

    /// The short's collateral, atoms.
    #[inline]
    #[must_use]
    pub const fn short_collateral(&self) -> u64 {
        self.short_collateral
    }

    /// `long_collateral + short_collateral`, atoms.
    ///
    /// # Errors
    ///
    /// [`CoreError::CollateralOverflow`] (checked at construction).
    #[inline]
    pub fn total(&self) -> Result<u64, CoreError> {
        self.long_collateral
            .checked_add(self.short_collateral)
            .ok_or(CoreError::CollateralOverflow)
    }

    /// `(long_payout, short_payout)`, summing exactly to [`SwapTerms::total`].
    ///
    /// Realized: `v = min(v_cons, cap)`; the long gets `long_collateral + floor(notional ×
    /// (v − strike))` when `v >= strike` and `long_collateral − ceil(notional × (strike −
    /// v))` otherwise, clamped to `[0, total]` as defense; the short gets the rest. Unwind:
    /// each side's collateral.
    ///
    /// # Errors
    ///
    /// [`CoreError::Overflow`] (unreachable within the validated terms).
    pub fn payouts(&self, s: Settlement) -> Result<(u64, u64), CoreError> {
        let total = self.total()?;
        let long = match s {
            Settlement::Unwind => self.long_collateral,
            Settlement::Realized(v_cons) => {
                let v = v_cons.min(self.cap_var);
                let long = u128::from(self.long_collateral);
                let long = if v >= self.strike_var {
                    let gain = mul_shr64(
                        self.notional,
                        v.checked_sub(self.strike_var).ok_or(CoreError::Overflow)?,
                        false,
                    )?;
                    long.checked_add(gain).ok_or(CoreError::Overflow)?
                } else {
                    let loss = mul_shr64(
                        self.notional,
                        self.strike_var.checked_sub(v).ok_or(CoreError::Overflow)?,
                        true,
                    )?;
                    // A loss beyond the collateral cannot happen within the cap (strike - v <= strike
                    // and long_collateral = ceil(notional x strike)); clamp at zero as defense.
                    if loss >= long {
                        0
                    } else {
                        long.checked_sub(loss).ok_or(CoreError::Overflow)?
                    }
                };
                u64::try_from(long.min(u128::from(total))).map_err(|_| CoreError::Overflow)?
            }
        };
        let short = total.checked_sub(long).ok_or(CoreError::Overflow)?;
        Ok((long, short))
    }
}
