//! Property tests against a reference model of the Pyth message tape (`docs/PLAN.md` 4.1).
//!
//! A tape is a sequence of successful aggregate times `T_0 <= T_1 <= …` (repeats are
//! aggregates within the same second) plus failed-slot copies, turned into messages
//! `(publish = T_i, prev = T_{i−1})`. The reference assigns each boundary its canonical print
//! by brute force; the window receives every message in a random order. Filled buckets,
//! prices, `sum_sq`, the final state and the realized variance must match exactly (integer
//! kernel, no tolerance).

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
use proptest::prelude::*;
use proptest::test_runner::{Config, RngSeed};
use varswap_core::{CoreError, Print, ReturnKernel, StoredPrice, Window, WindowSpec, WindowState};

/// Cases per property. `VARSWAP_PROPTEST_CASES` overrides the default.
fn config() -> Config {
    let cases = std::env::var("VARSWAP_PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4_000);
    Config {
        cases,
        rng_seed: RngSeed::Fixed(0x7A25_0A11_C0DE_0001),
        failure_persistence: None,
        ..Config::default()
    }
}

const START: i64 = 1_000_000;

#[derive(Debug, Clone)]
struct Tape {
    interval: u32,
    n: u32,
    tolerance: u32,
    /// `(publish, prev, price, exponent)` in tape order.
    messages: Vec<(i64, i64, u64, i32)>,
    /// Submission order keys.
    keys: Vec<u64>,
}

fn tape() -> impl Strategy<Value = Tape> {
    (1u32..=20, 1u32..=30, 1u32..=80).prop_flat_map(|(interval, n, tolerance)| {
        let steps = prop::collection::vec(
            (
                // Gap to the next aggregate: same second, short, or a long halt.
                prop_oneof![
                    3 => Just(0i64),
                    10 => 1i64..=i64::from(interval),
                    2 => 1i64..=i64::from(3 * tolerance + 3 * interval),
                ],
                1u64..=1_000,
                -4i32..=-1,
                // Failed-slot copies emitted after this aggregate.
                0usize..=2,
            ),
            1..=80,
        );
        let first = 1i64..=i64::from(2 * interval + 2);
        (
            Just((interval, n, tolerance)),
            first,
            steps,
            prop::collection::vec(any::<u64>(), 400),
        )
            .prop_map(|((interval, n, tolerance), first, steps, keys)| {
                let mut messages = Vec::new();
                // Aggregate 0 sits before the window; later ones follow the gaps.
                let mut prev = START - first;
                for (gap, price, exp, copies) in steps {
                    let publish = prev + gap;
                    messages.push((publish, prev, price, exp));
                    for _ in 0..copies {
                        // Failed slot: last trading price, prev == publish.
                        messages.push((publish, publish, price, exp));
                    }
                    if gap > 0 {
                        prev = publish;
                    }
                }
                Tape {
                    interval,
                    n,
                    tolerance,
                    messages,
                    keys,
                }
            })
    })
}

fn window_spec(t: &Tape) -> WindowSpec {
    spec(START, t.interval, t.n, t.tolerance, 3_600)
}

fn to_print(m: &(i64, i64, u64, i32)) -> Print {
    print(m.2, m.3, m.0, m.1)
}

/// Brute force: the bucket's canonical print from the definition, if within tolerance.
fn reference(t: &Tape, s: &WindowSpec) -> Vec<Option<StoredPrice>> {
    (0..=t.n)
        .map(|k| {
            let bk = s.boundary(k).unwrap().get();
            let hits: Vec<_> = t
                .messages
                .iter()
                .filter(|(publish, prev, _, _)| *prev < bk && bk <= *publish)
                .collect();
            // Canonicity: at most one print per boundary.
            assert!(hits.len() <= 1, "two canonical prints for b_{k}: {hits:?}");
            hits.first()
                .filter(|(publish, ..)| *publish <= bk + i64::from(t.tolerance))
                .map(|m| to_print(m).stored())
        })
        .collect()
}

fn reference_sum(buckets: &[Option<StoredPrice>]) -> u128 {
    buckets
        .windows(2)
        .filter_map(|w| match (w[0], w[1]) {
            (Some(a), Some(b)) => Some(ExactKernel.squared_log_return(a, b).unwrap().raw()),
            _ => None,
        })
        .sum()
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn prop_canonical_run_matches_brute_force(t in tape()) {
        let s = window_spec(&t);
        for m in &t.messages {
            let bf: Vec<u32> = (0..=t.n)
                .filter(|&k| {
                    let bk = s.boundary(k).unwrap().get();
                    m.1 < bk && bk <= m.0 && m.0 <= bk + i64::from(t.tolerance)
                })
                .collect();
            match s.canonical_run(&to_print(m)) {
                Ok(run) => {
                    let got: Vec<u32> = (run.first()..=run.last()).collect();
                    prop_assert_eq!(got, bf);
                    prop_assert!(
                        run.count().unwrap() <= t.tolerance / t.interval + 1
                    );
                }
                Err(e) => {
                    prop_assert_eq!(e, CoreError::NotOnGrid);
                    prop_assert!(bf.is_empty());
                }
            }
        }
    }

    #[test]
    fn prop_any_order_matches_reference(t in tape()) {
        let s = window_spec(&t);
        let want = reference(&t, &s);
        let mut order: Vec<usize> = (0..t.messages.len()).collect();
        order.sort_by_key(|&i| t.keys[i % t.keys.len()] ^ (i as u64));

        let mut data = vec![0u8; varswap_core::window_size(s.buckets().unwrap()).unwrap()];
        let mut w = Window::init(&mut data, s, 1).unwrap();
        let now = s.deadline().unwrap();
        for &i in &order {
            match w.sample(&to_print(&t.messages[i]), now, &ExactKernel) {
                Ok(_) | Err(CoreError::NotOnGrid) => {}
                Err(e) => prop_assert!(false, "message {i}: {e:?}"),
            }
        }
        // Replaying every message: nothing new is accepted.
        for m in &t.messages {
            let r = w.sample(&to_print(m), now, &ExactKernel);
            prop_assert!(
                matches!(r, Err(CoreError::NotOnGrid | CoreError::BucketAlreadySampled)),
                "replay accepted: {r:?}"
            );
        }
        for k in 0..=t.n {
            prop_assert_eq!(w.price(k).unwrap(), want[k as usize], "bucket {}", k);
        }
        let sum = reference_sum(&want);
        prop_assert_eq!(w.sum_sq().raw(), sum);
        let filled = want.iter().filter(|b| b.is_some()).count() as u32;
        prop_assert_eq!(w.filled(), filled);

        let after = varswap_core::UnixTimestamp::new(now.get() + 1);
        let state = w.finalize(after, &ExactKernel).unwrap();
        if filled == t.n + 1 {
            prop_assert_eq!(state, WindowState::Finalized);
            let span = u64::from(t.interval) * u64::from(t.n);
            prop_assert_eq!(
                w.realized_var().unwrap(),
                Some(ExactKernel.annualize(varswap_core::SumSq::from_raw(sum), span).unwrap())
            );
        } else {
            prop_assert_eq!(state, WindowState::Degraded);
            prop_assert_eq!(w.realized_var().unwrap(), None);
        }
        // The bytes reload to the same window.
        let w = Window::load(&mut data).unwrap();
        prop_assert_eq!(w.sum_sq().raw(), sum);
        prop_assert_eq!(w.filled(), filled);
    }
}

/// The tape strategy is not vacuous: over the configured cases it produces complete and
/// degraded windows, multi-bucket outage runs, prints the guard truncates, and same-second
/// or failed-slot copies.
#[test]
fn test_tape_strategy_covers_every_path() {
    use proptest::strategy::ValueTree;
    let mut runner = proptest::test_runner::TestRunner::new(config());
    let (mut finalized, mut degraded, mut multi, mut guarded, mut copies) = (0, 0, 0, 0, 0);
    for _ in 0..config().cases {
        let t = tape().new_tree(&mut runner).unwrap().current();
        let s = window_spec(&t);
        let want = reference(&t, &s);
        if want.iter().all(Option::is_some) {
            finalized += 1;
        } else {
            degraded += 1;
        }
        for m in &t.messages {
            if m.0 == m.1 {
                copies += 1;
            }
            if let Ok(run) = s.canonical_run(&to_print(m)) {
                if run.count().unwrap() > 1 {
                    multi += 1;
                }
                let crossed = (0..=t.n)
                    .filter(|&k| {
                        let bk = s.boundary(k).unwrap().get();
                        m.1 < bk && bk <= m.0
                    })
                    .count() as u32;
                if crossed > run.count().unwrap() {
                    guarded += 1;
                }
            }
        }
    }
    eprintln!(
        "finalized {finalized}, degraded {degraded}, multi-bucket runs {multi}, guarded {guarded}, copies {copies}"
    );
    for (name, n) in [
        ("finalized", finalized),
        ("degraded", degraded),
        ("multi", multi),
        ("guarded", guarded),
        ("copies", copies),
    ] {
        assert!(n >= 100, "{name}: {n}");
    }
}
