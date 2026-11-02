//! `varswap_core::SwapTerms` over its whole input space.
//!
//! Invariants for accepted terms: the collateral is `ceil(notional × strike)` and
//! `ceil(notional × (cap − strike))` (exact, in `u128` over `2^64`); `long + short == total`
//! exactly for every settlement; `Unwind` returns each collateral; the long payout never
//! decreases as the realized variance rises, is constant above the cap, and equals the long
//! collateral at the strike.

#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use varswap_core::{Seconds, Settlement, SwapTerms, MAX_CAP_VAR};

#[derive(Debug, Arbitrary)]
struct Input {
    notional: u64,
    strike: u128,
    cap: u128,
    interval: u32,
    v1: u128,
    v2: u128,
}

/// `ceil(n × v / 2^64)`, exact.
fn ceil_shr64(n: u64, v: u128) -> Option<u128> {
    // n × v < 2^64 × 2^128: split v in two halves to stay in u128.
    let (hi, lo) = (v >> 64, v & u128::from(u64::MAX));
    let lo_prod = u128::from(n) * lo;
    let hi_prod = u128::from(n).checked_mul(hi)?;
    let floor = hi_prod.checked_add(lo_prod >> 64)?;
    let rem = lo_prod & u128::from(u64::MAX);
    floor.checked_add(u128::from(rem != 0))
}

fuzz_target!(|i: Input| {
    // Bias toward the accepted region: strike and cap below MAX_CAP_VAR.
    let cap = i.cap % (MAX_CAP_VAR + 1);
    let strike = i.strike % cap.max(1);
    let Ok(t) = SwapTerms::try_new(i.notional, strike, cap, Seconds::new(i.interval)) else {
        return;
    };
    assert_eq!(
        Some(u128::from(t.long_collateral())),
        ceil_shr64(i.notional, strike)
    );
    assert_eq!(
        Some(u128::from(t.short_collateral())),
        ceil_shr64(i.notional, cap - strike)
    );
    let total = t.total().unwrap();
    assert_eq!(
        u128::from(total),
        u128::from(t.long_collateral()) + u128::from(t.short_collateral())
    );

    assert_eq!(
        t.payouts(Settlement::Unwind).unwrap(),
        (t.long_collateral(), t.short_collateral())
    );

    let (va, vb) = (i.v1.min(i.v2), i.v1.max(i.v2));
    let (la, sa) = t.payouts(Settlement::Realized(va)).unwrap();
    let (lb, sb) = t.payouts(Settlement::Realized(vb)).unwrap();
    assert_eq!(u128::from(la) + u128::from(sa), u128::from(total));
    assert_eq!(u128::from(lb) + u128::from(sb), u128::from(total));
    assert!(la <= lb, "long payout decreased: {la} > {lb}");

    let (ls, _) = t.payouts(Settlement::Realized(strike)).unwrap();
    assert_eq!(
        ls,
        t.long_collateral(),
        "at the strike the long gets its collateral"
    );
    let (lc, _) = t.payouts(Settlement::Realized(cap)).unwrap();
    if vb >= cap {
        assert_eq!(lb, lc, "payout moved above the cap");
    }
    assert!(lc <= total);
});
