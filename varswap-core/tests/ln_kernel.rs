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
    LnKernel, ReturnKernel, StoredPrice, UnixTimestamp, Window, WindowState, LN_10_RAW,
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
        let s = spec(1_000_000, interval, n, interval, 86_400);
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
