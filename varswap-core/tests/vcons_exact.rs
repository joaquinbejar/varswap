//! Direction of the conservative realized variance against a 100-digit decimal reference
//! (`tools/vcons_fixtures.py`, committed as `tests/fixtures/vcons.txt`): the f64 proptest in
//! `ln_kernel.rs` checks magnitudes only; the directional claims of `docs/PLAN.md` 4.4 are
//! checked here, exactly, on integers.

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

mod common;

use common::*;
use varswap_core::{LnKernel, Seconds, UnixTimestamp, Window, WindowState};

struct Case {
    interval: u32,
    cap: u128,
    notional: u128,
    eps: u128,
    /// floor(V_exact) in Q64.64 raw units.
    v_exact_floor: u128,
    /// floor(notional × min(V_exact, cap) / 2^64), atoms.
    payout_exact_floor: u128,
    prices: Vec<u64>,
}

fn cases() -> Vec<Case> {
    include_str!("fixtures/vcons.txt")
        .lines()
        .map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            Case {
                interval: f[0].parse().unwrap(),
                cap: f[1].parse().unwrap(),
                notional: f[2].parse().unwrap(),
                eps: f[3].parse().unwrap(),
                v_exact_floor: f[4].parse().unwrap(),
                payout_exact_floor: f[5].parse().unwrap(),
                prices: f[6..].iter().map(|p| p.parse().unwrap()).collect(),
            }
        })
        .collect()
}

/// The window's computed realized variance (exponent 0, one canonical print per boundary).
fn realized(c: &Case) -> u128 {
    let n = c.prices.len() as u32 - 1;
    let s = spec_exp(1_000_000, c.interval, n, c.interval, 86_400, 0);
    let mut data = vec![0u8; varswap_core::window_size(n + 1).unwrap()];
    let mut w = Window::init(&mut data, s, 0).unwrap();
    let now = s.deadline().unwrap();
    for (k, &p) in c.prices.iter().enumerate() {
        let bk = s.boundary(k as u32).unwrap().get();
        // Published at b_k with the previous aggregate 1 s earlier: exactly bucket k, even on a
        // 1 s grid.
        w.sample(&print(p, 0, bk, bk - 1), now, &LnKernel).unwrap();
    }
    assert_eq!(
        w.finalize(UnixTimestamp::new(now.get() + 1), &LnKernel)
            .unwrap(),
        WindowState::Finalized
    );
    w.realized_var().unwrap().unwrap()
}

/// The milestone 4 long payout with strike 1.0 and exact collateral:
/// floor(notional × min(V, cap) / 2^64).
fn long_payout(c: &Case, v: u128) -> u128 {
    (c.notional * v.min(c.cap)) >> 64
}

#[test]
fn test_vcons_direction_and_budget_against_decimal_reference() {
    let all = cases();
    assert!(all.len() > 600);
    let mut above_cap_uncapped_violations = 0;
    let mut max_advantage = 0;
    for (i, c) in all.iter().enumerate() {
        let eps = LnKernel::realized_var_error_bound(Seconds::new(c.interval), c.cap, 0).unwrap();
        assert_eq!(eps, c.eps, "case {i}: ε differs from the Python replica");
        let v = realized(c);
        // The computed variance is within ε of the exact one (|V − V_exact| <= ε, capped).
        let exact_capped = c.v_exact_floor.min(c.cap);
        assert!(
            v.min(c.cap).abs_diff(exact_capped) <= eps + 1,
            "case {i}: |min(V, cap) − min(V_exact, cap)| > ε"
        );
        let cons =
            LnKernel::conservative_realized_var(v, Seconds::new(c.interval), c.cap, 0).unwrap();
        // Guarantee: min(V_cons, cap) <= min(V_exact, cap) (integer vs floor: exact).
        assert!(
            cons.min(c.cap) <= exact_capped,
            "case {i}: capped V_cons above exact"
        );
        if cons > c.v_exact_floor {
            above_cap_uncapped_violations += 1;
            assert!(
                c.v_exact_floor >= c.cap,
                "case {i}: uncapped violation below the cap"
            );
        }
        // The long is never overpaid, and the short's advantage is under 3 atoms: the exact
        // payout is below payout_exact_floor + 1, so floor − payout <= 2.
        let long = long_payout(c, cons);
        assert!(long <= c.payout_exact_floor, "case {i}: long overpaid");
        let advantage = c.payout_exact_floor - long;
        assert!(
            advantage <= 2,
            "case {i}: short advantage {advantage} atoms"
        );
        max_advantage = max_advantage.max(advantage);
    }
    // The fixture exercises both edges of the argument.
    assert!(above_cap_uncapped_violations >= 1);
    assert_eq!(max_advantage, 2);
}

/// Codex #36470 (1): above the cap the uncapped V_cons exceeds V_exact (…6963 > …6955.86),
/// yet the capped guarantee holds.
#[test]
fn test_regression_above_cap_uncapped_direction_fails_capped_holds() {
    let c = cases().into_iter().nth(1).unwrap();
    assert_eq!(c.prices, [2_522, 7_638_331_619_254_237_224]);
    let v = realized(&c);
    let cons = LnKernel::conservative_realized_var(v, Seconds::new(c.interval), c.cap, 0).unwrap();
    assert_eq!(c.v_exact_floor, 23_440_309_410_282_258_976_955);
    assert_eq!(cons % 10_000, 6_963);
    assert!(cons > c.v_exact_floor, "uncapped V_cons <= V_exact here");
    assert!(cons.min(c.cap) <= c.v_exact_floor.min(c.cap));
    assert_eq!(long_payout(&c, cons), c.payout_exact_floor);
}

/// Codex #36470 (2): strike 1, the exact long payout floors to 76 and the conservative one to
/// 74: a 2-atom short advantage, inside the 3-atom budget.
#[test]
fn test_regression_two_atom_short_advantage_within_budget() {
    let c = cases().into_iter().nth(2).unwrap();
    assert_eq!(
        c.prices,
        [1_726_933_792_193_200_381, 1_726_933_815_967_915_399]
    );
    assert_eq!(c.notional, 401_016_175_515_425_035);
    let v = realized(&c);
    let cons = LnKernel::conservative_realized_var(v, Seconds::new(c.interval), c.cap, 0).unwrap();
    assert_eq!(c.payout_exact_floor, 76);
    assert_eq!(long_payout(&c, cons), 74);
}
