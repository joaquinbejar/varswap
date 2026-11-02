//! An arbitrary window and an arbitrary sequence of prints through `varswap_core::Window`
//! with the deployed kernel (`LnKernel`).
//!
//! Invariants after every step: no panic; a rejected print leaves the account byte for byte
//! unchanged; `filled` equals the number of filled buckets; a filled bucket never changes;
//! `sum_sq` equals the sum of the kernel's squared log returns over consecutive filled
//! buckets; `finalize` gives `Finalized` exactly when every bucket is filled, `Degraded`
//! only after the deadline, and otherwise `FinalizeTooEarly`.

#![no_main]

use std::num::NonZeroU64;

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use varswap_core::{
    window_size, Bps, CoreError, FeedId, LnKernel, Print, ReturnKernel, RiskParams,
    Seconds, StoredPrice, SumSq, UnixTimestamp, Window, WindowSpec, WindowState,
};

const FEED: [u8; 32] = [7; 32];
const START: i64 = 1_800_000_000;

#[derive(Debug, Arbitrary)]
struct Step {
    /// Price mantissa (zero is mapped to one).
    price: u64,
    /// 1: the exponent is one above the pinned one (rejected); anything else: the pinned one.
    exponent_off: u8,
    /// Seconds after `START - interval`, so prints before the window are reachable.
    publish_off: u32,
    /// `publish - prev` (prev = 0 is rejected by `Print`).
    gap: u32,
    /// Another feed's message.
    wrong_feed: bool,
    /// `now` for this sample, seconds after `START`.
    now_off: u32,
}

#[derive(Debug, Arbitrary)]
struct Input {
    interval: u16,
    intervals: u8,
    tolerance: u16,
    backfill: u16,
    exponent: i8,
    steps: Vec<Step>,
    finalize_now_off: u32,
}

fn spec(i: &Input) -> Option<WindowSpec> {
    let interval = u32::from(i.interval.max(1));
    let n = u32::from(i.intervals.max(1));
    let end = START + i64::from(interval) * i64::from(n);
    WindowSpec::try_new(
        FeedId::new(FEED),
        UnixTimestamp::new(START),
        UnixTimestamp::new(end),
        Seconds::new(interval),
        RiskParams {
            tolerance: Seconds::new(u32::from(i.tolerance.max(1))),
            max_conf: Bps::new(1_000),
            backfill_deadline: Seconds::new(u32::from(i.backfill.max(1))),
            receiver_program: [9; 32],
            exponent: i.exponent,
        },
    )
    .ok()
}

/// Every filled bucket's price, and the reference `sum_sq` over consecutive filled buckets.
fn snapshot(w: &Window<'_>, n: u32) -> (Vec<Option<StoredPrice>>, Option<SumSq>) {
    let prices: Vec<_> = (0..n).map(|k| w.price(k).unwrap()).collect();
    let mut sum = SumSq::from_raw(0);
    for pair in prices.windows(2) {
        if let (Some(a), Some(b)) = (pair[0], pair[1]) {
            match LnKernel.squared_log_return(a, b) {
                Ok(t) => sum = sum.checked_add(t).unwrap(),
                // A pair the kernel rejects could not have been written by `sample`.
                Err(_) => return (prices, None),
            }
        }
    }
    (prices, Some(sum))
}

fuzz_target!(|input: Input| {
    let Some(spec) = spec(&input) else {
        return;
    };
    let n = spec.buckets().unwrap();
    let mut data = vec![0u8; window_size(n).unwrap()];
    Window::init(&mut data, spec, 255).unwrap();
    let interval = i64::from(spec.interval().get());

    for s in &input.steps {
        let publish = START - interval + i64::from(s.publish_off);
        let prev = publish - i64::from(s.gap);
        let feed = if s.wrong_feed { [8; 32] } else { FEED };
        let exponent = i32::from(spec.exponent()) + i32::from(s.exponent_off == 1);
        let price = NonZeroU64::new(s.price.max(1)).unwrap();
        let Ok(print) = Print::try_new(
            FeedId::new(feed),
            price,
            exponent,
            UnixTimestamp::new(publish),
            UnixTimestamp::new(prev),
        ) else {
            continue;
        };
        let now = UnixTimestamp::new(START + i64::from(s.now_off));

        let before = data.clone();
        let mut w = Window::load(&mut data).unwrap();
        let (old_prices, _) = snapshot(&w, n);
        let old_missing = w.missing().unwrap();
        match w.sample(&print, now, &LnKernel) {
            Err(_) => assert_eq!(data, before, "a rejected print wrote the account"),
            Ok(run) => {
                let w = Window::load(&mut data).unwrap();
                let (prices, sum) = snapshot(&w, n);
                for k in 0..n {
                    let in_run = run.first() <= k && k <= run.last();
                    if in_run {
                        assert_eq!(old_prices[k as usize], None, "refilled bucket {k}");
                        assert_eq!(prices[k as usize], Some(print.stored()));
                    } else {
                        assert_eq!(prices[k as usize], old_prices[k as usize], "bucket {k} changed");
                    }
                }
                assert_eq!(
                    w.missing().unwrap(),
                    old_missing - run.count().unwrap(),
                    "missing count"
                );
                let filled = prices.iter().filter(|p| p.is_some()).count() as u32;
                assert_eq!(w.missing().unwrap(), n - filled);
                assert_eq!(sum, Some(w.sum_sq()), "sum_sq is not the sum of the returns");
                assert_eq!(w.state().unwrap(), WindowState::Sampling);
            }
        }
    }

    let before = data.clone();
    let mut w = Window::load(&mut data).unwrap();
    let missing = w.missing().unwrap();
    let now = UnixTimestamp::new(START + i64::from(input.finalize_now_off));
    let after_deadline = now > spec.deadline().unwrap();
    match w.finalize(now, &LnKernel) {
        Ok(WindowState::Finalized) => {
            assert_eq!(missing, 0);
            assert!(w.realized_var().unwrap().is_some());
        }
        Ok(WindowState::Degraded) => {
            assert!(missing > 0 && after_deadline);
            assert_eq!(w.realized_var().unwrap(), None);
        }
        Ok(s) => panic!("finalize returned {s:?}"),
        Err(CoreError::FinalizeTooEarly) => {
            assert!(missing > 0 && !after_deadline);
            assert_eq!(data, before);
        }
        // The annualization of a complete window can overflow only for absurd sums.
        Err(e) => {
            assert_eq!(missing, 0, "finalize failed with {e:?}");
            assert_eq!(data, before);
        }
    }
});
