//! Swap terms, collateral, the error-bound gate and settlement: conservation in proptest
//! across realized variance in `[0, 2 × cap]` (`docs/PLAN.md` milestone 4).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::disallowed_macros,
    missing_docs
)]

use proptest::prelude::*;
use proptest::test_runner::{Config, RngSeed};
use varswap_core::{CoreError, LnKernel, Seconds, Settlement, SwapTerms, MAX_CAP_VAR};

const ONE: u128 = 1 << 64;
const H: Seconds = Seconds::new(3_600);

fn config() -> Config {
    let cases = std::env::var("VARSWAP_PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20_000);
    Config {
        cases,
        rng_seed: RngSeed::Fixed(0x7A25_0A11_C0DE_0004),
        failure_persistence: None,
        ..Config::default()
    }
}

fn max_notional(interval: Seconds, cap: u128) -> u64 {
    let eps = LnKernel::realized_var_error_bound(interval, cap, 0).unwrap();
    u64::try_from((ONE - 1) / eps).unwrap()
}

#[test]
fn test_terms_rejections() {
    let cap = 25 * ONE;
    assert_eq!(
        SwapTerms::try_new(0, ONE, cap, H),
        Err(CoreError::ZeroNotional)
    );
    assert_eq!(
        SwapTerms::try_new(1, 0, cap, H),
        Err(CoreError::InvalidStrike)
    );
    assert_eq!(
        SwapTerms::try_new(1, ONE, ONE, H),
        Err(CoreError::InvalidCap)
    );
    assert_eq!(
        SwapTerms::try_new(1, 2 * ONE, ONE, H),
        Err(CoreError::InvalidCap)
    );
    assert_eq!(
        SwapTerms::try_new(1, ONE, MAX_CAP_VAR + 1, H),
        Err(CoreError::InvalidCap)
    );
    assert!(SwapTerms::try_new(1, ONE, MAX_CAP_VAR, H).is_ok());
}

#[test]
fn test_error_gate_at_limit_and_above() {
    for interval in [1u32, 60, 3_600, 86_400] {
        let iv = Seconds::new(interval);
        for cap in [2 * ONE, 25 * ONE, 400 * ONE] {
            let n = max_notional(iv, cap);
            let strike = ONE;
            match SwapTerms::try_new(n, strike, cap, iv) {
                Ok(_) | Err(CoreError::CollateralOverflow) => {}
                Err(e) => panic!("limit rejected at {interval} s: {e:?}"),
            }
            assert_eq!(
                SwapTerms::try_new(n + 1, strike, cap, iv),
                Err(CoreError::NotionalAboveErrorBound),
                "{interval} s, cap {}",
                cap >> 64
            );
        }
    }
}

#[test]
fn test_collateral_rounds_up_exactly() {
    // notional 3, strike 1/3 (raw floor): 3 × floor(2^64 / 3) / 2^64 is just under 1.
    let t = SwapTerms::try_new(3, ONE / 3, ONE, H).unwrap();
    assert_eq!(t.long_collateral(), 1);
    // 3 × (2^64 − floor(2^64 / 3)) / 2^64 is just over 2.
    assert_eq!(t.short_collateral(), 3);
    // Exact products need no round-up.
    let t = SwapTerms::try_new(1_000, ONE, 5 * ONE / 2, H).unwrap();
    assert_eq!((t.long_collateral(), t.short_collateral()), (1_000, 1_500));
}

#[test]
fn test_unwind_returns_each_collateral() {
    let t = SwapTerms::try_new(1_000_000, ONE / 25, ONE, H).unwrap();
    assert_eq!(
        t.payouts(Settlement::Unwind).unwrap(),
        (t.long_collateral(), t.short_collateral())
    );
}

#[test]
fn test_payout_extremes() {
    let t = SwapTerms::try_new(1_000_000, ONE / 25, 2 * ONE, H).unwrap();
    let total = t.total().unwrap();
    // Zero realized variance: the long loses its whole collateral.
    assert_eq!(t.payouts(Settlement::Realized(0)).unwrap(), (0, total));
    // At or above the cap: the short loses its collateral except the rounding it keeps
    // (collateral rounded up, the long's gain rounded down: at most one atom here).
    for v in [2 * ONE, u128::MAX] {
        let (long, short) = t.payouts(Settlement::Realized(v)).unwrap();
        assert_eq!(long + short, total);
        assert!(short <= 1, "short kept {short}");
    }
    // At the strike: the long gets its collateral back exactly.
    assert_eq!(
        t.payouts(Settlement::Realized(ONE / 25)).unwrap().0,
        t.long_collateral()
    );
}

proptest! {
    #![proptest_config(config())]

    /// Conservation and monotonicity across realized variance in [0, 2 × cap], at notionals
    /// up to the gate: payouts sum to the collateral exactly, never exceed it, and the long's
    /// payout never decreases as the variance rises.
    #[test]
    fn prop_conservation_and_monotone(
        strike_f in 1u64..=1_000_000,
        width in 1u64..=1_000_000,
        frac in 0u64..=1_000_000,
        iv in prop_oneof![Just(1u32), Just(60), Just(3_600), Just(86_400)],
        v1 in 0u64..=2_000_000,
        v2 in 0u64..=2_000_000,
    ) {
        // strike in (0, 10], cap in (strike, strike + 10] (variance units), raw.
        let strike = u128::from(strike_f) * (ONE / 100_000);
        let cap = strike + u128::from(width) * (ONE / 100_000);
        let iv = Seconds::new(iv);
        let n_max = max_notional(iv, cap);
        let notional = 1 + (u128::from(n_max - 1) * u128::from(frac) / 1_000_000) as u64;
        let t = match SwapTerms::try_new(notional, strike, cap, iv) {
            Ok(t) => t,
            Err(CoreError::CollateralOverflow) => return Ok(()),
            Err(e) => return Err(TestCaseError::fail(format!("{e:?}"))),
        };
        let total = t.total().unwrap();
        let (va, vb) = (
            2 * cap * u128::from(v1.min(v2)) / 2_000_000,
            2 * cap * u128::from(v1.max(v2)) / 2_000_000,
        );
        let (la, sa) = t.payouts(Settlement::Realized(va)).unwrap();
        let (lb, sb) = t.payouts(Settlement::Realized(vb)).unwrap();
        prop_assert_eq!(la + sa, total);
        prop_assert_eq!(lb + sb, total);
        prop_assert!(la <= lb, "long payout decreased: {} > {}", la, lb);
        // The long's gain never exceeds notional × (v − strike) (floor), and its loss is
        // never less than notional × (strike − v) (ceil): checked in exact integers as
        // (payout − collateral) × 2^64 <= notional × (v − strike).
        let v = va.min(cap);
        let lhs = (i128::from(la) - i128::from(t.long_collateral())) * (1i128 << 64);
        let rhs = i128::try_from(u128::from(notional) * v).ok()
            .zip(i128::try_from(u128::from(notional) * strike).ok());
        if let Some((a, b)) = rhs {
            if la > 0 && la < total {
                prop_assert!(lhs <= a - b, "long paid above notional × (v − strike)");
            }
        }
        let (lu, su) = t.payouts(Settlement::Unwind).unwrap();
        prop_assert_eq!((lu, su), (t.long_collateral(), t.short_collateral()));
    }
}
