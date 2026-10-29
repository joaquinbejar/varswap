//! Layout sizes, spec validation, canonical runs, sampling, finalize and account validation,
//! with a negative test per rule.

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

use core::mem::{align_of, offset_of, size_of};
use std::num::NonZeroU64;

use common::*;
use varswap_core::layout::{Hdr, WindowHeader, WindowSpecRaw};
use varswap_core::{
    window_size, Bps, CoreError, Print, RiskParams, Seconds, Window, WindowSpec, WindowState,
    CPI_ACCOUNT_CREATION_LIMIT, MAX_BUCKETS, WINDOW_TAG,
};

const START: i64 = 1_800_000_000;
const H: u32 = 3_600;

fn hourly(n: u32) -> WindowSpec {
    // 6 h outage guard, 24 h backfill: the v1 parameters.
    spec(START, H, n, 6 * H, 24 * H)
}

fn account(s: &WindowSpec) -> Vec<u8> {
    vec![0u8; window_size(s.buckets().unwrap()).unwrap()]
}

fn b(s: &WindowSpec, k: u32) -> i64 {
    s.boundary(k).unwrap().get()
}

/// The canonical print of boundary `k` of an hourly grid: published 1 s after it.
fn at(s: &WindowSpec, k: u32, price: u64) -> Print {
    print(price, -8, b(s, k) + 1, b(s, k) - 1)
}

// ---- layout and size bound (3.1) -------------------------------------------------------

#[test]
fn test_layout_sizes_offsets_and_alignment() {
    assert_eq!(size_of::<Hdr>(), 8);
    assert_eq!(size_of::<WindowSpecRaw>(), 96);
    assert_eq!(size_of::<WindowHeader>(), 144);
    assert_eq!(align_of::<WindowHeader>(), 8);
    // WindowSpecRaw ordered by alignment: the only padding is the trailing `_pad`.
    assert_eq!(offset_of!(WindowSpecRaw, feed), 0);
    assert_eq!(offset_of!(WindowSpecRaw, start), 32);
    assert_eq!(offset_of!(WindowSpecRaw, end), 40);
    assert_eq!(offset_of!(WindowSpecRaw, receiver_program), 48);
    assert_eq!(offset_of!(WindowSpecRaw, interval), 80);
    assert_eq!(offset_of!(WindowSpecRaw, tolerance), 84);
    assert_eq!(offset_of!(WindowSpecRaw, backfill_deadline), 88);
    assert_eq!(offset_of!(WindowSpecRaw, max_conf), 92);
    assert_eq!(offset_of!(WindowSpecRaw, _pad), 94);
    assert_eq!(offset_of!(WindowHeader, spec), 8);
    assert_eq!(offset_of!(WindowHeader, state), 104);
    assert_eq!(offset_of!(WindowHeader, filled), 108);
    assert_eq!(offset_of!(WindowHeader, sum_sq), 112);
    assert_eq!(offset_of!(WindowHeader, realized_var), 128);
}

#[test]
fn test_window_size_formula_and_max_buckets_bound() {
    for b in [1u32, 2, 63, 64, 65, 721, 1_104, 1_105, 1_106] {
        let words = (b as usize).div_ceil(64);
        assert_eq!(window_size(b).unwrap(), 144 + 8 * words + 9 * b as usize);
    }
    assert_eq!(window_size(721).unwrap(), 6_729);
    assert_eq!(window_size(MAX_BUCKETS).unwrap(), 10_233);
    assert_eq!(window_size(MAX_BUCKETS + 1).unwrap(), 10_242);
    assert!(window_size(MAX_BUCKETS).unwrap() <= CPI_ACCOUNT_CREATION_LIMIT);
    assert!(CPI_ACCOUNT_CREATION_LIMIT < window_size(MAX_BUCKETS + 1).unwrap());
}

#[test]
fn test_spec_max_buckets_accepted_one_more_rejected() {
    assert_eq!(hourly(MAX_BUCKETS - 1).buckets().unwrap(), MAX_BUCKETS);
    let r = WindowSpec::try_new(
        FEED,
        t(START),
        t(START + i64::from(H) * i64::from(MAX_BUCKETS)),
        Seconds::new(H),
        risk(6 * H, 24 * H),
    );
    assert_eq!(r, Err(CoreError::TooManyBuckets));
}

// ---- spec validation (3.3) -------------------------------------------------------------

fn try_spec(start: i64, end: i64, interval: u32, r: RiskParams) -> Result<WindowSpec, CoreError> {
    WindowSpec::try_new(FEED, t(start), t(end), Seconds::new(interval), r)
}

#[test]
fn test_spec_rejections() {
    let ok = risk(6 * H, 24 * H);
    assert_eq!(try_spec(START, START, H, ok), Err(CoreError::EmptyWindow));
    assert_eq!(
        try_spec(START, START - 1, H, ok),
        Err(CoreError::EmptyWindow)
    );
    assert_eq!(
        try_spec(START, START + 10, 0, ok),
        Err(CoreError::ZeroInterval)
    );
    assert_eq!(
        try_spec(START, START + i64::from(H) + 1, H, ok),
        Err(CoreError::EndOffGrid)
    );
    assert_eq!(
        try_spec(START, START + i64::from(H), H, risk(0, 24 * H)),
        Err(CoreError::ZeroTolerance)
    );
    assert_eq!(
        try_spec(START, START + i64::from(H), H, risk(H, 0)),
        Err(CoreError::ZeroBackfillDeadline)
    );
    for bad in [0u16, 10_001, u16::MAX] {
        let r = RiskParams {
            max_conf: Bps::new(bad),
            ..ok
        };
        assert_eq!(
            try_spec(START, START + i64::from(H), H, r),
            Err(CoreError::InvalidMaxConf)
        );
    }
    assert!(try_spec(
        START,
        START + i64::from(H),
        H,
        RiskParams {
            max_conf: Bps::new(10_000),
            ..ok
        }
    )
    .is_ok());
    // end + backfill_deadline must be representable.
    assert_eq!(
        try_spec(i64::MAX - 2, i64::MAX - 1, 1, risk(1, 2)),
        Err(CoreError::Overflow)
    );
    // N = 1 is the smallest window.
    assert_eq!(hourly(1).buckets().unwrap(), 2);
}

#[test]
fn test_spec_check_creatable() {
    let s = hourly(24);
    assert!(s.check_creatable(t(START - 1), 25).is_ok());
    assert_eq!(
        s.check_creatable(t(START), 25),
        Err(CoreError::StartNotInFuture)
    );
    assert_eq!(
        s.check_creatable(t(START + 1), 25),
        Err(CoreError::StartNotInFuture)
    );
    assert_eq!(
        s.check_creatable(t(START - 1), 24),
        Err(CoreError::TooManyBuckets)
    );
}

#[test]
fn test_spec_raw_roundtrip_and_padding_checked() {
    let s = hourly(720);
    assert_eq!(WindowSpec::from_raw(&s.to_raw()), Ok(s));
    let mut raw = s.to_raw();
    raw._pad = [0, 1];
    assert_eq!(
        WindowSpec::from_raw(&raw),
        Err(CoreError::InvalidAccountData)
    );
    let mut raw = s.to_raw();
    raw.interval = 0;
    assert_eq!(WindowSpec::from_raw(&raw), Err(CoreError::ZeroInterval));
}

// ---- prints ----------------------------------------------------------------------------

#[test]
fn test_print_exponent_outside_i8_rejected() {
    let p = |e| Print::try_new(FEED, NonZeroU64::new(1).unwrap(), e, t(10), t(9));
    assert!(p(-128).is_ok());
    assert!(p(127).is_ok());
    assert_eq!(p(-129), Err(CoreError::ExponentOutOfRange));
    assert_eq!(p(128), Err(CoreError::ExponentOutOfRange));
    assert_eq!(p(i32::MIN), Err(CoreError::ExponentOutOfRange));
}

#[test]
fn test_print_prev_publish_time_zero_rejected() {
    let r = Print::try_new(FEED, NonZeroU64::new(1).unwrap(), -8, t(10), t(0));
    assert_eq!(r, Err(CoreError::NoPreviousPrint));
}

#[test]
fn test_print_price_above_i64_rejected() {
    let r = Print::try_new(FEED, NonZeroU64::new(u64::MAX).unwrap(), -8, t(10), t(9));
    assert_eq!(r, Err(CoreError::PriceOutOfRange));
}

#[test]
fn test_print_from_verified_mainnet_fixture() {
    let data = include_bytes!("../../pyth-lite/fixtures/sol_usd.data.bin");
    let owner = include_bytes!("../../pyth-lite/fixtures/sol_usd.owner.bin");
    let u = pyth_lite::PriceUpdate::load(owner, data, &pyth_lite::RECEIVER_PROGRAM_ID).unwrap();
    let feed = u.message().feed_id();
    let req = pyth_lite::PriceRequirements::new(feed, None, 1_000);
    let o = u.verify(&req, u.message().publish_time()).unwrap();
    let p = Print::try_from(&o).unwrap();
    assert_eq!(p.feed_id(), feed);
    assert_eq!(p.stored().exponent(), -8);
    assert_eq!(p.stored().price(), u.message().price());
    assert_eq!(p.publish_time(), u.message().publish_time());
    assert_eq!(p.prev_publish_time(), u.message().prev_publish_time());
}

// ---- canonical runs (4.1) --------------------------------------------------------------

#[test]
fn test_canonical_run_b0_uses_euclidean_floor() {
    let s = hourly(24);
    // prev − start = −1: floor is −1 (k_lo = 0); truncation would give 0 (k_lo = 1).
    let run = s.canonical_run(&print(1, -8, START, START - 1)).unwrap();
    assert_eq!((run.first(), run.last()), (0, 0));
    let run = s
        .canonical_run(&print(1, -8, START + 5, START - 3_601))
        .unwrap();
    assert_eq!((run.first(), run.last()), (0, 0));
}

#[test]
fn test_canonical_run_boundaries_inclusive_exclusive() {
    let s = hourly(24);
    let b3 = b(&s, 3);
    // prev == b_k excludes b_k; publish == b_k includes it.
    assert_eq!(
        s.canonical_run(&print(1, -8, b3, b3)),
        Err(CoreError::NotOnGrid)
    );
    let run = s.canonical_run(&print(1, -8, b3, b3 - 1)).unwrap();
    assert_eq!((run.first(), run.last()), (3, 3));
    // publish == b_k + tolerance still fills b_k; one second later does not.
    let tol = i64::from(s.tolerance().get());
    let run = s.canonical_run(&print(1, -8, b3 + tol, b3 - 1)).unwrap();
    assert_eq!(run.first(), 3);
    let run = s
        .canonical_run(&print(1, -8, b3 + tol + 1, b3 - 1))
        .unwrap();
    assert_eq!(run.first(), 4);
}

#[test]
fn test_canonical_run_outage_spans_several_boundaries() {
    let s = hourly(24);
    // Halt from just before b_1 until 5 s after b_4: one print fills buckets 1..=4.
    let run = s
        .canonical_run(&print(1, -8, b(&s, 4) + 5, b(&s, 1) - 10))
        .unwrap();
    assert_eq!((run.first(), run.last()), (1, 4));
    assert_eq!(run.count().unwrap(), 4);
}

#[test]
fn test_canonical_run_outage_guard_drops_old_boundaries() {
    let s = hourly(24); // tolerance 6 h
                        // Halt of 10 h from before b_1: only b_5..=b_10 are within 6 h of the reopening print.
    let run = s
        .canonical_run(&print(1, -8, b(&s, 10) + 5, b(&s, 1) - 10))
        .unwrap();
    assert_eq!((run.first(), run.last()), (5, 10));
}

#[test]
fn test_canonical_run_off_grid_rejected() {
    let s = hourly(24);
    let cases = [
        // Same-second or failed-slot copy: empty interval.
        print(1, -8, b(&s, 2) + 9, b(&s, 2) + 9),
        // Interval between two boundaries.
        print(1, -8, b(&s, 2) + 9, b(&s, 2) + 1),
        // Entirely before the window.
        print(1, -8, START - 1, START - 100),
        // Entirely after the window.
        print(1, -8, b(&s, 24) + 100, b(&s, 24)),
        // prev after publish.
        print(1, -8, b(&s, 2) + 1, b(&s, 3)),
    ];
    for p in cases {
        assert_eq!(s.canonical_run(&p), Err(CoreError::NotOnGrid), "{p:?}");
    }
}

#[test]
fn test_canonical_run_clamped_to_last_bucket() {
    let s = hourly(24);
    let run = s
        .canonical_run(&print(1, -8, b(&s, 24) + 10, b(&s, 23) + 1))
        .unwrap();
    assert_eq!((run.first(), run.last()), (24, 24));
}

#[test]
fn test_boundary_out_of_range() {
    let s = hourly(24);
    assert_eq!(b(&s, 0), START);
    assert_eq!(b(&s, 24), START + 24 * i64::from(H));
    assert_eq!(s.boundary(25), Err(CoreError::NotOnGrid));
}

// ---- sampling (3.3) --------------------------------------------------------------------

#[test]
fn test_init_rejects_wrong_size_and_reinit() {
    let s = hourly(24);
    let mut short = vec![0u8; window_size(25).unwrap() - 1];
    assert!(matches!(
        Window::init(&mut short, s, 255),
        Err(CoreError::InvalidAccountData)
    ));
    let mut long = vec![0u8; window_size(25).unwrap() + 1];
    assert!(matches!(
        Window::init(&mut long, s, 255),
        Err(CoreError::InvalidAccountData)
    ));
    let mut data = account(&s);
    Window::init(&mut data, s, 254).unwrap();
    assert!(matches!(
        Window::init(&mut data, s, 254),
        Err(CoreError::InvalidAccountData)
    ));
    let w = Window::load(&mut data).unwrap();
    assert_eq!(w.bump(), 254);
    assert_eq!(w.state().unwrap(), WindowState::Pending);
    assert_eq!(w.missing().unwrap(), 25);
    assert_eq!(w.realized_var().unwrap(), None);
}

#[test]
fn test_sample_fills_and_adds_edge_returns_only() {
    let s = hourly(24);
    let mut data = account(&s);
    let mut w = Window::init(&mut data, s, 1).unwrap();
    let now = t(b(&s, 24));
    w.sample(&at(&s, 0, 100), now, &ExactKernel).unwrap();
    assert_eq!(w.sum_sq().raw(), 0);
    assert_eq!(w.state().unwrap(), WindowState::Sampling);
    // Outage over b_1..=b_4: one print, one return over 4 intervals.
    let run = w
        .sample(
            &print(130, -8, b(&s, 4) + 5, b(&s, 1) - 10),
            now,
            &ExactKernel,
        )
        .unwrap();
    assert_eq!((run.first(), run.last()), (1, 4));
    assert_eq!(w.sum_sq().raw(), 30 * 30);
    // Bucket 6, then 5 between two filled neighbours: both edges.
    w.sample(&at(&s, 6, 120), now, &ExactKernel).unwrap();
    assert_eq!(w.sum_sq().raw(), 900);
    w.sample(&at(&s, 5, 110), now, &ExactKernel).unwrap();
    assert_eq!(w.sum_sq().raw(), 900 + 20 * 20 + 10 * 10);
    assert_eq!(w.filled(), 7);
    for k in 1..=4 {
        assert_eq!(w.price(k).unwrap().unwrap().price(), 130);
    }
    assert_eq!(w.price(7).unwrap(), None);
}

#[test]
fn test_sample_exponents_kept_per_bucket() {
    let s = hourly(2);
    let mut data = account(&s);
    let mut w = Window::init(&mut data, s, 1).unwrap();
    let now = t(b(&s, 2));
    w.sample(
        &print(1_000, -3, b(&s, 0) + 1, b(&s, 0) - 1),
        now,
        &ExactKernel,
    )
    .unwrap();
    // Same price 1.0 written with another exponent: zero return.
    w.sample(
        &print(100_000, -5, b(&s, 1) + 1, b(&s, 1) - 1),
        now,
        &ExactKernel,
    )
    .unwrap();
    assert_eq!(w.sum_sq().raw(), 0);
    assert_eq!(w.price(0).unwrap().unwrap().exponent(), -3);
    assert_eq!(w.price(1).unwrap().unwrap().exponent(), -5);
}

#[test]
fn test_sample_replay_rejected_and_bytes_untouched() {
    let s = hourly(24);
    let mut data = account(&s);
    let p = at(&s, 3, 100);
    let now = t(b(&s, 24));
    Window::init(&mut data, s, 1)
        .unwrap()
        .sample(&p, now, &ExactKernel)
        .unwrap();
    let before = data.clone();
    let mut w = Window::load(&mut data).unwrap();
    assert_eq!(
        w.sample(&p, now, &ExactKernel),
        Err(CoreError::BucketAlreadySampled)
    );
    // An overlapping outage print whose run includes the filled bucket.
    let q = print(100, -8, b(&s, 4) + 1, b(&s, 2) - 1);
    assert_eq!(
        w.sample(&q, now, &ExactKernel),
        Err(CoreError::BucketAlreadySampled)
    );
    assert_eq!(data, before);
}

#[test]
fn test_sample_rejections_leave_bytes_untouched() {
    let s = hourly(24);
    let mut data = account(&s);
    Window::init(&mut data, s, 1).unwrap();
    let before = data.clone();
    let deadline = s.deadline().unwrap().get();
    let mut w = Window::load(&mut data).unwrap();
    let other = Print::try_new(
        OTHER_FEED,
        NonZeroU64::new(1).unwrap(),
        -8,
        t(b(&s, 1) + 1),
        t(b(&s, 1) - 1),
    )
    .unwrap();
    assert_eq!(
        w.sample(&other, t(deadline), &ExactKernel),
        Err(CoreError::WrongFeed)
    );
    assert_eq!(
        w.sample(&at(&s, 1, 1), t(deadline + 1), &ExactKernel),
        Err(CoreError::BackfillClosed)
    );
    assert_eq!(
        w.sample(
            &print(1, -8, b(&s, 1) + 5, b(&s, 1) + 5),
            t(deadline),
            &ExactKernel
        ),
        Err(CoreError::NotOnGrid)
    );
    assert_eq!(data, before);
    // At the deadline itself the sample is accepted.
    let mut w = Window::load(&mut data).unwrap();
    assert!(w.sample(&at(&s, 1, 1), t(deadline), &ExactKernel).is_ok());
}

// ---- finalize and unwind triggers (4.1, 4.3) -------------------------------------------

#[test]
fn test_finalize_all_filled_immediately() {
    let s = hourly(3);
    let mut data = account(&s);
    let mut w = Window::init(&mut data, s, 1).unwrap();
    let now = t(b(&s, 3) + 2);
    for (k, p) in [(0, 100), (1, 110), (2, 90), (3, 100)] {
        w.sample(&at(&s, k, p), now, &ExactKernel).unwrap();
    }
    let sum = 100 + 400 + 100;
    assert_eq!(w.sum_sq().raw(), sum);
    // Long before the deadline: a complete window finalizes at once.
    assert_eq!(
        w.finalize(now, &ExactKernel).unwrap(),
        WindowState::Finalized
    );
    let span = u128::from(3 * H);
    assert_eq!(w.realized_var().unwrap(), Some(sum * 31_536_000 / span));
    assert_eq!(w.finalize(now, &ExactKernel), Err(CoreError::WindowFinal));
    assert_eq!(
        w.sample(&at(&s, 0, 1), now, &ExactKernel),
        Err(CoreError::WindowFinal)
    );
}

#[test]
fn test_finalize_incomplete_only_after_deadline() {
    let s = hourly(3);
    let mut data = account(&s);
    let mut w = Window::init(&mut data, s, 1).unwrap();
    let deadline = s.deadline().unwrap().get();
    for k in [0, 1, 3] {
        w.sample(&at(&s, k, 100), t(deadline), &ExactKernel)
            .unwrap();
    }
    assert_eq!(w.missing().unwrap(), 1);
    assert_eq!(
        w.finalize(t(deadline), &ExactKernel),
        Err(CoreError::FinalizeTooEarly)
    );
    assert_eq!(
        w.finalize(t(deadline + 1), &ExactKernel).unwrap(),
        WindowState::Degraded
    );
    assert_eq!(w.realized_var().unwrap(), None);
    assert_eq!(
        w.finalize(t(deadline + 2), &ExactKernel),
        Err(CoreError::WindowFinal)
    );
}

#[test]
fn test_finalize_empty_window_degrades() {
    let s = hourly(3);
    let mut data = account(&s);
    let mut w = Window::init(&mut data, s, 1).unwrap();
    let deadline = s.deadline().unwrap().get();
    assert_eq!(
        w.finalize(t(START), &ExactKernel),
        Err(CoreError::FinalizeTooEarly)
    );
    assert_eq!(
        w.finalize(t(deadline + 1), &ExactKernel).unwrap(),
        WindowState::Degraded
    );
}

#[test]
fn test_outage_longer_than_guard_degrades() {
    // A 10 h halt starting before b_1: the reopening print can fill only b_5..=b_10, so
    // b_1..=b_4 have no canonical print and the window unwinds.
    let s = hourly(12);
    let mut data = account(&s);
    let mut w = Window::init(&mut data, s, 1).unwrap();
    let deadline = s.deadline().unwrap().get();
    w.sample(&at(&s, 0, 100), t(deadline), &ExactKernel)
        .unwrap();
    w.sample(
        &print(100, -8, b(&s, 10) + 5, b(&s, 1) - 10),
        t(deadline),
        &ExactKernel,
    )
    .unwrap();
    w.sample(&at(&s, 11, 100), t(deadline), &ExactKernel)
        .unwrap();
    w.sample(&at(&s, 12, 100), t(deadline), &ExactKernel)
        .unwrap();
    assert_eq!(w.missing().unwrap(), 4);
    for k in 1..=4 {
        assert!(!w.is_filled(k).unwrap());
    }
    assert_eq!(
        w.finalize(t(deadline + 1), &ExactKernel).unwrap(),
        WindowState::Degraded
    );
}

// ---- account validation on load --------------------------------------------------------

fn sampled() -> (WindowSpec, Vec<u8>) {
    let s = hourly(70); // 71 buckets: two bitmap words, 7 tail bits unused.
    let mut data = account(&s);
    let mut w = Window::init(&mut data, s, 1).unwrap();
    let now = t(b(&s, 70) + 2);
    for k in [0, 1, 64, 70] {
        w.sample(&at(&s, k, 100 + u64::from(k)), now, &ExactKernel)
            .unwrap();
    }
    (s, data)
}

type Mutation = (&'static str, Box<dyn Fn(&mut Vec<u8>)>);

fn load_err(data: &mut [u8]) -> CoreError {
    match Window::load(data) {
        Err(e) => e,
        Ok(_) => panic!("loaded a corrupted window"),
    }
}

#[test]
fn test_load_valid_window_roundtrip() {
    let (_, mut data) = sampled();
    let w = Window::load(&mut data).unwrap();
    assert_eq!(w.filled(), 4);
    assert_eq!(w.price(64).unwrap().unwrap().price(), 164);
}

#[test]
fn test_load_rejects_corruption() {
    let (s, data) = sampled();
    let bitmap = 144;
    let prices = bitmap + 16;
    let mutations: Vec<Mutation> = vec![
        ("truncated", Box::new(|d| d.truncate(d.len() - 1))),
        ("extended", Box::new(|d| d.push(0))),
        ("header only", Box::new(|d| d.truncate(100))),
        ("tag", Box::new(|d| d[0] = 2)),
        ("uninitialized", Box::new(|d| d[0] = 0)),
        ("version", Box::new(|d| d[1] = 2)),
        ("hdr pad", Box::new(|d| d[3] = 1)),
        ("spec pad", Box::new(|d| d[8 + 94] = 1)),
        (
            "spec interval",
            Box::new(|d| d[8 + 80..8 + 84].copy_from_slice(&0u32.to_le_bytes())),
        ),
        ("state value", Box::new(|d| d[104] = 4)),
        ("state finalized", Box::new(|d| d[104] = 2)),
        ("state pending", Box::new(|d| d[104] = 0)),
        ("header pad", Box::new(|d| d[105] = 1)),
        ("filled", Box::new(|d| d[108] = 5)),
        ("extra bit", Box::new(move |d| d[bitmap] |= 0b100)),
        ("tail bit", Box::new(move |d| d[bitmap + 15] |= 0x80)),
    ];
    for (name, m) in mutations {
        let mut d = data.clone();
        m(&mut d);
        let e = load_err(&mut d);
        assert!(matches!(e, CoreError::InvalidAccountData), "{name}: {e:?}");
    }
    // A filled bucket holding a non-positive price is rejected when read.
    for bad in [0i64, -1] {
        let mut d = data.clone();
        let at = prices + 8 * 64;
        d[at..at + 8].copy_from_slice(&bad.to_le_bytes());
        let w = Window::load(&mut d).unwrap();
        assert_eq!(w.price(64), Err(CoreError::InvalidAccountData));
        assert_eq!(w.price(2).unwrap(), None);
    }
    // A wrong spec size for the data: shrink N in the stored spec.
    let mut d = data.clone();
    let end = s.end().get() - i64::from(H);
    d[8 + 40..8 + 48].copy_from_slice(&end.to_le_bytes());
    assert_eq!(load_err(&mut d), CoreError::InvalidAccountData);
    // The state byte a well-formed Degraded window would carry still loads.
    let mut d = data.clone();
    d[104] = 3;
    assert!(Window::load(&mut d).is_ok());
    assert_eq!(d[0], WINDOW_TAG);
}
