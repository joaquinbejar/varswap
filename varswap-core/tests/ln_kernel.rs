//! The optfix `ln` kernel against f64 references: single terms, exponent changes, extreme
//! exponents, and realized variance on simulated GBM paths within the documented bound
//! (`docs/PLAN.md` milestone 1b).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::float_arithmetic,
    clippy::cast_precision_loss,
    clippy::disallowed_macros,
    missing_docs
)]

mod common;

use common::*;
use optfix::Fixed;
use proptest::prelude::*;
use proptest::test_runner::{Config, RngSeed};
use varswap_core::{
    LnKernel, ReturnKernel, Seconds, StoredPrice, UnixTimestamp, Window, WindowState, LN_10_RAW,
    YEAR_SECONDS,
};

const TWO_64: f64 = 18_446_744_073_709_551_616.0;

fn config() -> Config {
    let cases = std::env::var("VARSWAP_PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2_000);
    Config {
        cases,
        rng_seed: RngSeed::Fixed(0x7A25_0A11_C0DE_0002),
        failure_persistence: None,
        ..Config::default()
    }
}

fn sp(price: i64, exponent: i8) -> StoredPrice {
    StoredPrice::try_new(price, exponent).unwrap()
}

/// f64 reference of `ln(to / from)` from exact integer mantissas: `ln_1p` of the exact
/// difference over `from` keeps the relative error near `2^-52` even for tiny returns.
fn ref_log_return(from: i64, to: i64) -> f64 {
    ((to - from) as f64 / from as f64).ln_1p()
}

#[test]
fn test_ln10_constant_matches_optfix_ln() {
    let ln10 = optfix::ln(Fixed::from_int(10)).unwrap().raw();
    assert!((ln10 - LN_10_RAW).abs() <= i128::from(optfix::LN_MAX_ERROR_ULPS));
}

#[test]
fn test_same_price_other_exponent_zero_return() {
    // 1.0 written as 1_000e-3 and as 100_000e-5.
    let r = LnKernel::log_return(sp(1_000, -3), sp(100_000, -5)).unwrap();
    let bound = LnKernel::term_error_bound(r, 2).unwrap();
    assert!(r.raw().unsigned_abs() <= bound / 2 + 1, "r = {}", r.raw());
    let sq = LnKernel
        .squared_log_return(sp(1_000, -3), sp(100_000, -5))
        .unwrap();
    assert!(sq.raw() <= bound);
}

#[test]
fn test_extreme_exponents_do_not_overflow() {
    // A ratio of 10^255 would not fit Q64.64; the ln-difference form does.
    let r = LnKernel::log_return(sp(1, -128), sp(1, 127)).unwrap();
    let want = 255.0 * std::f64::consts::LN_10;
    assert!((r.raw() as f64 / TWO_64 - want).abs() < 1e-12);
    let sq = LnKernel
        .squared_log_return(sp(1, -128), sp(1, 127))
        .unwrap();
    assert!((sq.raw() as f64 / TWO_64 - want * want).abs() < 1e-9);
    // The largest mantissas.
    assert!(LnKernel::log_return(sp(1, 0), sp(i64::MAX, 0)).is_ok());
}

#[test]
fn test_squared_return_rounds_down_and_is_symmetric() {
    let up = LnKernel.squared_log_return(sp(100, 0), sp(110, 0)).unwrap();
    let down = LnKernel.squared_log_return(sp(110, 0), sp(100, 0)).unwrap();
    // 1.1 is not exact in f64: take the reference from the integer prices.
    let r = ref_log_return(100, 110);
    let want = r * r * TWO_64;
    for got in [up.raw(), down.raw()] {
        let bound = LnKernel::term_error_bound(Fixed::from_raw((r * TWO_64) as i128), 0).unwrap();
        assert!(
            (got as f64 - want).abs() <= bound as f64 + want * 1e-15,
            "{got} vs {want}"
        );
    }
}

/// splitmix64 for the path noise.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn uniform(&mut self) -> f64 {
        ((self.next() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    /// Standard normal, Box–Muller.
    fn normal(&mut self) -> f64 {
        let (u, v) = (self.uniform(), self.uniform());
        (-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos()
    }
}

/// A GBM path of `n + 1` prices sampled every `interval` seconds, as Pyth mantissas with
/// exponent `exp`: `S_{k+1} = S_k exp((μ − σ²/2) Δt + σ √Δt Z)`.
fn gbm(seed: u64, s0: f64, mu: f64, sigma: f64, interval: u32, n: u32, exp: i8) -> Vec<i64> {
    let mut rng = Rng(seed);
    let dt = f64::from(interval) / YEAR_SECONDS as f64;
    let scale = 10f64.powi(-i32::from(exp));
    let mut s = s0;
    let mut out = Vec::with_capacity(n as usize + 1);
    for _ in 0..=n {
        out.push(((s * scale).round() as i64).max(1));
        s *= ((mu - sigma * sigma / 2.0) * dt + sigma * dt.sqrt() * rng.normal()).exp();
    }
    out
}

proptest! {
    #![proptest_config(config())]

    /// On GBM paths every squared log return is within the documented bound of the f64
    /// reference, the window's `sum_sq` and realized variance are exactly the sum of those
    /// terms and its annualization, and a single missing bucket degrades the window.
    #[test]
    fn prop_gbm_realized_variance_within_bound(
        seed in any::<u64>(),
        sigma in 0.05f64..3.0,
        mu in -1.0f64..1.0,
        interval in prop_oneof![Just(60u32), Just(900), Just(3_600), Just(86_400)],
        n in 2u32..=720,
        (s0, exp) in prop_oneof![
            Just((150.0f64, -8i8)),
            Just((4_000.0, -3)),
            Just((62.0, -5)),
            Just((0.000_02, -10)),
        ],
        missing in any::<u32>(),
    ) {
        let prices = gbm(seed, s0, mu, sigma, interval, n, exp);
        let s = spec_exp(1_000_000, interval, n, interval, 86_400, exp);
        let mut data = vec![0u8; varswap_core::window_size(n + 1).unwrap()];
        let mut w = Window::init(&mut data, s, 0).unwrap();
        let now = s.deadline().unwrap();
        for (k, &p) in prices.iter().enumerate() {
            let bk = s.boundary(k as u32).unwrap().get();
            w.sample(&print(p as u64, i32::from(exp), bk + 1, bk - 1), now, &LnKernel).unwrap();
        }
        // Each term against f64 at near-ulp resolution: the reference's own error is about
        // 4 × 2^-53 relative (ln_1p, the division, the square), so the tolerance is the
        // kernel bound plus that, in raw units.
        let mut sum = 0u128;
        let mut worst = 0f64;
        for pair in prices.windows(2) {
            let k = LnKernel
                .squared_log_return(sp(pair[0], exp), sp(pair[1], exp))
                .unwrap()
                .raw();
            let r = ref_log_return(pair[0], pair[1]);
            let want = r * r * TWO_64;
            let bound = LnKernel::term_error_bound(Fixed::from_raw((r * TWO_64) as i128), 0)
                .unwrap() as f64;
            let tol = bound + 4.0 * f64::EPSILON * want + 1.0;
            let err = (k as f64 - want).abs();
            prop_assert!(err <= tol, "term {} vs {}: error {} > {}", k, want, err, tol);
            worst = worst.max(err - bound);
            sum += k;
        }
        // The window sums the same terms exactly, and annualizes exactly (rounded down).
        prop_assert_eq!(w.sum_sq().raw(), sum);
        let state = w.finalize(UnixTimestamp::new(now.get() + 1), &LnKernel).unwrap();
        prop_assert_eq!(state, WindowState::Finalized);
        let span = u128::from(interval) * u128::from(n);
        prop_assert_eq!(
            w.realized_var().unwrap(),
            Some(sum * u128::from(YEAR_SECONDS) / span)
        );
        prop_assert!(worst.is_finite());

        // The conservative variance against the exact one. The exact sum is the per-term f64
        // references (each within 4 × 2^-53 relative, already checked above) summed with
        // Neumaier compensation and given that error in the unfavourable direction; the
        // closed-form ε also dominates the per-term bounds it is derived from.
        let realized = w.realized_var().unwrap().unwrap();
        let cap = (25u128) << 64;
        let eps = LnKernel::realized_var_error_bound(Seconds::new(interval), cap, 0).unwrap();
        let cons =
            LnKernel::conservative_realized_var(realized, Seconds::new(interval), cap, 0)
                .unwrap();
        let (mut hi, mut comp) = (0f64, 0f64);
        let mut term_bounds = 0f64;
        for pair in prices.windows(2) {
            let r = ref_log_return(pair[0], pair[1]);
            let x = r * r * TWO_64 * (1.0 + 4.0 * f64::EPSILON);
            let t = hi + x;
            comp += if hi.abs() >= x.abs() { (hi - t) + x } else { (x - t) + hi };
            hi = t;
            term_bounds += LnKernel::term_error_bound(Fixed::from_raw((r * TWO_64) as i128), 0)
                .unwrap() as f64;
        }
        let exact_hi = (hi + comp) * YEAR_SECONDS as f64 / span as f64;
        let exact_v = exact_hi / TWO_64;
        if exact_v <= 25.0 {
            prop_assert!(
                term_bounds * YEAR_SECONDS as f64 / span as f64 + 1.0 <= eps as f64,
                "closed form {} below the per-term sum {}", eps, term_bounds
            );
        }
        // The guarantee is on the capped values: ε(cap) bounds |min(V, cap) − min(V_exact,
        // cap)|, so min(V_cons, cap) <= min(V_exact, cap). Above the cap the uncapped
        // V_cons <= V_exact need not hold.
        let cap_f = cap as f64;
        prop_assert!(
            (cons.min(cap) as f64) <= exact_hi.min(cap_f) * (1.0 + 2.0 * f64::EPSILON),
            "min(V_cons, cap) {} > min(V_exact, cap) {}", cons.min(cap), exact_hi.min(cap_f)
        );
        // The budget (PLAN 4.4): at the largest notional the limit allows (notional × ε <
        // 2^64), the long is never overpaid and the short's advantage is under 3 atoms
        // (under 2 from ±ε then −ε, at most 1 from the floor). Strike 1.0, collateral exact.
        let notional = (u128::MAX >> 64) / eps;
        let long_payout = (notional * cons.min(cap)) >> 64;
        let exact_payout = notional as f64 * exact_hi.min(cap_f) / TWO_64;
        // f64 resolution of the exact payout: the reference's relative error times its size.
        let res = exact_payout * 8.0 * f64::EPSILON + 1e-6;
        let advantage = exact_payout - long_payout as f64;
        prop_assert!(advantage >= -res, "long overpaid by {} atoms", -advantage);
        prop_assert!(advantage < 3.0 + res, "short advantage {} atoms >= 3", advantage);
        prop_assert!(cons <= realized && realized - cons <= eps);

        // Any gap degrades the window.
        let gap = missing % (n + 1);
        let mut data = vec![0u8; varswap_core::window_size(n + 1).unwrap()];
        let mut w = Window::init(&mut data, s, 0).unwrap();
        for (k, &p) in prices.iter().enumerate() {
            if k as u32 == gap {
                continue;
            }
            let bk = s.boundary(k as u32).unwrap().get();
            w.sample(&print(p as u64, i32::from(exp), bk + 1, bk - 1), now, &LnKernel).unwrap();
        }
        prop_assert_eq!(
            w.finalize(UnixTimestamp::new(now.get() + 1), &LnKernel).unwrap(),
            WindowState::Degraded
        );
        prop_assert_eq!(w.realized_var().unwrap(), None);
    }
}

#[test]
fn test_gbm_estimates_sigma_squared() {
    // 30 days hourly at σ = 80%: realized variance within 4 standard errors of σ².
    let prices = gbm(42, 150.0, 0.0, 0.8, 3_600, 720, -8);
    let s = spec(1_000_000, 3_600, 720, 3_600, 86_400);
    let mut data = vec![0u8; varswap_core::window_size(721).unwrap()];
    let mut w = Window::init(&mut data, s, 0).unwrap();
    let now = s.deadline().unwrap();
    for (k, &p) in prices.iter().enumerate() {
        let bk = s.boundary(k as u32).unwrap().get();
        w.sample(&print(p as u64, -8, bk + 1, bk - 1), now, &LnKernel)
            .unwrap();
    }
    w.finalize(UnixTimestamp::new(now.get() + 1), &LnKernel)
        .unwrap();
    let var = w.realized_var().unwrap().unwrap() as f64 / TWO_64;
    let se = 0.64 * (2.0f64 / 720.0).sqrt();
    assert!((var - 0.64).abs() < 4.0 * se, "realized {var}");
}

/// `realized_var_error_bound` matches its closed form and covers the actual error of complete
/// windows on GBM paths below the cap. A sanity check only: over a whole window the f64
/// reference's own error is larger than the bound; the tight check is the per-term proptest,
/// from which the closed form follows by the derivation in the function's docs.
#[test]
fn test_realized_var_error_bound_covers_gbm_windows() {
    let two64 = TWO_64;
    for (interval, n, sigma) in [
        (60u32, 1_104u32, 3.0f64),
        (1, 1_104, 1.0),
        (3_600, 720, 0.8),
    ] {
        let cap = 25.0f64; // 500% vol squared
        let cap_raw = (cap * two64) as u128;
        let bound = LnKernel::realized_var_error_bound(Seconds::new(interval), cap_raw, 0).unwrap();
        // Closed form in f64.
        let y = (YEAR_SECONDS as f64 / f64::from(interval)).ceil();
        let closed = 2.0 * 4.0 * (cap * y).sqrt() + 3.0 * y + 2.0;
        assert!((bound as f64 - closed).abs() <= 3.0, "{bound} vs {closed}");
        for seed in 0..20u64 {
            let prices = gbm(seed, 150.0, 0.0, sigma, interval, n, -8);
            let mut sum = 0u128;
            let mut exact = 0f64;
            for pair in prices.windows(2) {
                sum += LnKernel
                    .squared_log_return(sp(pair[0], -8), sp(pair[1], -8))
                    .unwrap()
                    .raw();
                let r = ref_log_return(pair[0], pair[1]);
                exact += r * r;
            }
            let span = u128::from(interval) * u128::from(n);
            let got = (sum * u128::from(YEAR_SECONDS) / span) as f64;
            let want = exact * two64 * YEAR_SECONDS as f64 / span as f64;
            if want > cap * two64 {
                continue;
            }
            // f64 slack: the reference's own relative error over the sum.
            let slack = want * f64::from(n + 4) * f64::EPSILON;
            assert!(
                (got - want).abs() <= bound as f64 + slack,
                "interval {interval} seed {seed}: error {} > bound {bound} + {slack}",
                (got - want).abs()
            );
        }
    }
}

#[test]
fn test_realized_var_error_bound_worst_case_numbers() {
    // The values PLAN 4.4 quotes: cap 25 (500% vol), the exponent pinned (Δe = 0).
    let cap_raw = 25u128 << 64;
    let b60 = LnKernel::realized_var_error_bound(Seconds::new(60), cap_raw, 0).unwrap();
    let b1 = LnKernel::realized_var_error_bound(Seconds::new(1), cap_raw, 0).unwrap();
    let b3600 = LnKernel::realized_var_error_bound(Seconds::new(3_600), cap_raw, 0).unwrap();
    // Max notional (atoms per unit variance) with error under one atom: 2^64 / ε.
    let n60 = (1u128 << 64) / b60;
    let n1 = (1u128 << 64) / b1;
    let n3600 = (1u128 << 64) / b3600;
    eprintln!("bound 60 s {b60} -> {n60}; 1 s {b1} -> {n1}; 3600 s {b3600} -> {n3600}");
    assert_eq!((b60, n60), (1_605_802, 11_487_558_287_827));
    assert_eq!((b1, n1), (94_832_630, 194_518_954_854));
    assert_eq!((b3600, n3600), (30_026, 614_359_024_635_634));
    // Unpinned (Δe up to 255), for comparison.
    let u60 = LnKernel::realized_var_error_bound(Seconds::new(60), cap_raw, 255).unwrap();
    assert_eq!(u60, 3_454_508);
    assert_eq!(
        LnKernel::realized_var_error_bound(Seconds::new(0), cap_raw, 0),
        Err(varswap_core::CoreError::ZeroInterval)
    );
}

/// Codex's counterexample (coordinator #36437): a one-year interval, `N = 1`, exponent 0.
/// The computed variance is 0.2 raw units above the exact one, and that is enough for the
/// floored long payout to cross an atom boundary: +1 atom against the short on `V`, none on
/// `V_cons = max(0, V − ε)`.
#[test]
fn test_regression_ln_error_crosses_atom_boundary_without_v_cons() {
    const P0: u64 = 2_501_942_101_381_778_875;
    const P1: u64 = 5_161_686_394_897_806_096;
    const NOTIONAL: u128 = 399_829_521_432_443_555; // atoms per unit variance
    const STRIKE: u128 = 1 << 64; // strike variance 1.0
    const CAP: u128 = 25 << 64;
    // Exact V raw = 9_674_578_593_639_773_023.798… (80-digit decimal); the exact long payout
    // floor(NOTIONAL × V_exact / 2^64):
    const EXACT_PAYOUT: u128 = 209_694_573_400_002_746;

    let year = YEAR_SECONDS as u32;
    let s = spec_exp(1_000_000, year, 1, year, 86_400, 0);
    let mut data = vec![0u8; varswap_core::window_size(2).unwrap()];
    let mut w = Window::init(&mut data, s, 0).unwrap();
    let now = s.deadline().unwrap();
    for (k, p) in [(0, P0), (1, P1)] {
        let bk = s.boundary(k).unwrap().get();
        w.sample(&print(p, 0, bk + 1, bk - 1), now, &LnKernel)
            .unwrap();
    }
    w.finalize(UnixTimestamp::new(now.get() + 1), &LnKernel)
        .unwrap();
    let v = w.realized_var().unwrap().unwrap();
    assert_eq!(v, 9_674_578_593_639_773_024);
    let eps = LnKernel::realized_var_error_bound(Seconds::new(year), CAP, 0).unwrap();
    assert_eq!(eps, 46);

    // The milestone 4 payout: long collateral NOTIONAL × STRIKE (exact here), plus
    // NOTIONAL × (min(V, cap) − STRIKE), floored; with STRIKE = 1 that is
    // floor(NOTIONAL × min(V, cap) / 2^64).
    let long_payout = |var: u128| {
        let v = var.min(CAP);
        let collateral = NOTIONAL * STRIKE;
        (collateral + NOTIONAL * v - NOTIONAL * STRIKE) >> 64
    };
    // On the computed V the long gets one atom more than on the exact variance.
    assert_eq!(long_payout(v), EXACT_PAYOUT + 1);
    // On V_cons it never exceeds the exact payout.
    let cons = LnKernel::conservative_realized_var(v, Seconds::new(year), CAP, 0).unwrap();
    assert_eq!(cons, v - 46);
    assert!(long_payout(cons) <= EXACT_PAYOUT);
    assert_eq!(long_payout(cons), EXACT_PAYOUT);
}
