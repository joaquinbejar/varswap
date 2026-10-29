//! `create_window`, `sample` and `finalize` in LiteSVM, with a forged-account negative test
//! per rule of `docs/PLAN.md` 3.3, the `FeedConfig` binding, and a 24-sample replay checked
//! against the off-chain accumulator.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::disallowed_macros,
    clippy::result_large_err,
    missing_docs
)]

mod common;

use std::num::NonZeroU64;

use common::*;
use solana_address::Address;
use solana_instruction::AccountMeta;
use solana_signer::Signer;
use varswap_core::{
    window_size, Bps, LnKernel, Print, ReturnKernel, RiskParams, Seconds, UnixTimestamp, Window,
    WindowSpec, WindowState,
};
use varswap_program::layout::SampleResult;

const START: i64 = T0 + H as i64;

fn b(k: u32) -> i64 {
    START + i64::from(H) * i64::from(k)
}

/// The canonical print of `b_k`: published 1 s after it, previous aggregate 1 s before.
fn at(k: u32, price: i64) -> PriceUpdate {
    PriceUpdate::sol(price, b(k) + 1, b(k) - 1)
}

fn load(env: &Env, w: &Address) -> Vec<u8> {
    env.account(w).data
}

fn state(env: &Env, w: &Address) -> WindowState {
    let mut d = load(env, w);
    Window::load(&mut d).unwrap().state().unwrap()
}

// ---- create_window ---------------------------------------------------------------------

#[test]
fn test_create_window_copies_feed_parameters() {
    let (env, w) = Env::with_window(24);
    let acc = env.account(&w);
    assert_eq!(acc.owner, PROGRAM_ID);
    assert_eq!(acc.data.len(), window_size(25).unwrap());
    let mut d = acc.data;
    let win = Window::load(&mut d).unwrap();
    let s = win.spec();
    assert_eq!(s.feed().to_bytes(), SOL);
    assert_eq!(s.tolerance().get(), TOLERANCE);
    assert_eq!(s.backfill_deadline().get(), BACKFILL);
    assert_eq!(s.max_conf().get(), MAX_CONF);
    assert_eq!(s.receiver_program(), pyth_lite::RECEIVER_PROGRAM_ID);
    assert_eq!(win.state().unwrap(), WindowState::Pending);
}

#[test]
fn test_create_window_max_buckets_fits_cpi_cap() {
    // B = MAX_BUCKETS = 1,105: 10,233 bytes created through the system program CPI.
    let mut env = Env::new();
    env.init_config().unwrap();
    let mut a = feed_args(SOL);
    a.max_buckets = varswap_core::MAX_BUCKETS;
    env.set_feed(a).unwrap();
    let n = varswap_core::MAX_BUCKETS - 1;
    let end = START + i64::from(H) * i64::from(n);
    env.create_window(SOL, START, end, H).unwrap();
    let acc = env.account(&window_pda(SOL, START, end, H));
    assert_eq!(acc.data.len(), 10_233);
    // One more bucket is rejected by the spec before any CPI.
    let end = end + i64::from(H);
    expect_code(
        &env.create_window(SOL, START, end, H),
        code::TOO_MANY_BUCKETS,
    );
}

#[test]
fn test_create_window_rule_rejections() {
    let (mut env, _) = Env::with_window(24);
    let day = START + 24 * i64::from(H);
    let cases: [(&str, i64, i64, u32, u32); 7] = [
        ("start == end", START, START, H, code::EMPTY_WINDOW),
        ("interval 0", START, day, 0, code::ZERO_INTERVAL),
        ("end off grid", START, day + 1, H, code::END_OFF_GRID),
        (
            "start not in future",
            T0,
            T0 + i64::from(H),
            H,
            code::START_NOT_IN_FUTURE,
        ),
        (
            "start in the past",
            T0 - 1,
            T0 - 1 + i64::from(H),
            H,
            code::START_NOT_IN_FUTURE,
        ),
        (
            "interval below feed minimum",
            START,
            START + 30,
            30,
            code::INTERVAL_BELOW_MINIMUM,
        ),
        (
            "more buckets than the feed allows",
            START,
            START + i64::from(H) * i64::from(MAX_BUCKETS),
            H,
            code::TOO_MANY_BUCKETS,
        ),
    ];
    for (name, s, e, iv, want) in cases {
        assert_eq!(err_code(&env.create_window(SOL, s, e, iv)), want, "{name}");
    }
}

#[test]
fn test_create_window_twice_rejected() {
    let (mut env, _) = Env::with_window(24);
    let end = START + 24 * i64::from(H);
    expect_code(
        &env.create_window(SOL, START, end, H),
        code::ALREADY_INITIALIZED,
    );
}

#[test]
fn test_create_window_unvetted_feed_rejected() {
    // No FeedConfig for XAU: the PDA is an empty system account.
    let (mut env, _) = Env::with_window(24);
    let end = START + 24 * i64::from(H);
    expect_code(
        &env.create_window(XAU, START, end, H),
        code::INVALID_ACCOUNT_OWNER,
    );
}

#[test]
fn test_create_window_forged_feed_config_rejected() {
    let (mut env, _) = Env::with_window(24);
    let end = START + 2 * i64::from(H);
    let real = env.account(&feed_pda(SOL));
    for (owner, want) in [
        (Address::new_unique(), code::INVALID_ACCOUNT_OWNER),
        (PROGRAM_ID, code::INVALID_PDA),
    ] {
        let forged = Address::new_unique();
        env.set(forged, owner, real.data.clone());
        let mut ix = ix_create_window(&env.payer.pubkey(), SOL, START, end, H);
        ix.accounts[1] = AccountMeta::new_readonly(forged, false);
        let p = env.payer.insecure_clone();
        expect_code(&env.send(&[ix], &[&p]), want);
    }
    // A FeedConfig with a looser tolerance planted at the real PDA but another tag.
    let mut data = real.data.clone();
    data[0] = 1;
    env.set(feed_pda(SOL), PROGRAM_ID, data);
    expect_code(
        &env.create_window(SOL, START, end, H),
        code::INVALID_ACCOUNT_DATA,
    );
}

#[test]
fn test_create_window_wrong_window_pda_rejected() {
    let (mut env, _) = Env::with_window(24);
    let end = START + 2 * i64::from(H);
    let mut ix = ix_create_window(&env.payer.pubkey(), SOL, START, end, H);
    // The PDA of another spec.
    ix.accounts[2] = AccountMeta::new(window_pda(SOL, START, end + i64::from(H), H), false);
    let p = env.payer.insecure_clone();
    expect_code(&env.send(&[ix], &[&p]), code::INVALID_PDA);
}

#[test]
fn test_create_window_bad_system_program_and_readonly_window_rejected() {
    let (mut env, _) = Env::with_window(24);
    let end = START + 2 * i64::from(H);
    let mut ix = ix_create_window(&env.payer.pubkey(), SOL, START, end, H);
    ix.accounts[3] = AccountMeta::new_readonly(Address::new_unique(), false);
    let p = env.payer.insecure_clone();
    expect_code(&env.send(&[ix], &[&p]), code::INVALID_SYSTEM_PROGRAM);
    let mut ix = ix_create_window(&env.payer.pubkey(), SOL, START, end, H);
    ix.accounts[2].is_writable = false;
    expect_code(&env.send(&[ix], &[&p]), code::NOT_WRITABLE);
}

// ---- sample ----------------------------------------------------------------------------

#[test]
fn test_sample_fills_bucket_and_returns_run() {
    let (mut env, w) = Env::with_window(24);
    env.set_time(b(0) + 5);
    let meta = env.sample(&w, &at(0, 100_000_000)).unwrap();
    let r: SampleResult = bytemuck::pod_read_unaligned(&meta.return_data.data);
    assert_eq!((r.first, r.last), (0, 0));
    assert_eq!(r.price, 100_000_000);
    assert_eq!(r.publish_time, b(0) + 1);
    assert_eq!(state(&env, &w), WindowState::Sampling);
}

#[test]
fn test_sample_outage_one_print_fills_run() {
    let (mut env, w) = Env::with_window(24);
    env.set_time(b(5));
    env.sample(&w, &at(0, 100_000_000)).unwrap();
    // Halt from before b_1 to 5 s after b_4.
    let meta = env
        .sample(&w, &PriceUpdate::sol(130_000_000, b(4) + 5, b(1) - 10))
        .unwrap();
    let r: SampleResult = bytemuck::pod_read_unaligned(&meta.return_data.data);
    assert_eq!((r.first, r.last), (1, 4));
    let mut d = load(&env, &w);
    let win = Window::load(&mut d).unwrap();
    assert_eq!(win.filled(), 5);
    // One return ln(1.3) over the four intervals b_0..b_4, the run's inner returns zero.
    let want = varswap_core::LnKernel
        .squared_log_return(
            varswap_core::StoredPrice::try_new(100_000_000, -8).unwrap(),
            varswap_core::StoredPrice::try_new(130_000_000, -8).unwrap(),
        )
        .unwrap();
    assert_eq!(win.sum_sq(), want);
}

#[test]
fn test_sample_late_backfill_until_deadline() {
    let (mut env, w) = Env::with_window(24);
    let deadline = b(24) + i64::from(BACKFILL);
    env.set_time(deadline);
    env.sample(&w, &at(3, 100_000_000)).unwrap();
    env.set_time(deadline + 1);
    expect_code(&env.sample(&w, &at(4, 100_000_000)), code::BACKFILL_CLOSED);
}

#[test]
fn test_sample_oracle_rejections() {
    let (mut env, w) = Env::with_window(24);
    env.set_time(b(2));
    let good = at(1, 100_000_000);
    let cases: Vec<(&str, PriceUpdate, u32)> = vec![
        (
            "partial verification",
            PriceUpdate {
                partial: Some(13),
                ..good
            },
            code::ORACLE_NOT_FULL,
        ),
        (
            "other feed",
            PriceUpdate { feed: XAU, ..good },
            code::ORACLE_WRONG_FEED,
        ),
        (
            "zero price",
            PriceUpdate {
                price: 0,
                conf: 0,
                ..good
            },
            code::ORACLE_NON_POSITIVE,
        ),
        (
            "negative price",
            PriceUpdate {
                price: -5,
                conf: 0,
                ..good
            },
            code::ORACLE_NON_POSITIVE,
        ),
        (
            "confidence over 10%",
            PriceUpdate {
                conf: 10_000_001,
                ..good
            },
            code::ORACLE_CONF,
        ),
        (
            "exponent outside i8",
            PriceUpdate {
                exponent: -200,
                ..good
            },
            code::EXPONENT_OUT_OF_RANGE,
        ),
        (
            "prev_publish_time 0",
            PriceUpdate {
                prev_publish_time: 0,
                ..good
            },
            code::NO_PREVIOUS_PRINT,
        ),
        (
            "same-second copy",
            PriceUpdate::sol(100_000_000, b(1) + 3, b(1) + 3),
            code::NOT_ON_GRID,
        ),
        (
            "between boundaries",
            PriceUpdate::sol(100_000_000, b(1) + 30, b(1) + 3),
            code::NOT_ON_GRID,
        ),
    ];
    for (name, u, want) in cases {
        assert_eq!(err_code(&env.sample(&w, &u)), want, "{name}");
    }
    // Exactly 10% confidence passes.
    env.sample(
        &w,
        &PriceUpdate {
            conf: 10_000_000,
            ..good
        },
    )
    .unwrap();
}

#[test]
fn test_sample_price_update_forged_owner_rejected() {
    let (mut env, w) = Env::with_window(24);
    env.set_time(b(2));
    let at_addr = Address::new_unique();
    env.set(at_addr, Address::new_unique(), at(1, 100_000_000).bytes());
    expect_code(&env.sample_at(&w, &at_addr), code::ORACLE_WRONG_OWNER);
    // Owned by the push oracle program (not the receiver).
    env.set(
        at_addr,
        Address::new_from_array(pyth_lite::PUSH_ORACLE_PROGRAM_ID),
        at(1, 100_000_000).bytes(),
    );
    expect_code(&env.sample_at(&w, &at_addr), code::ORACLE_WRONG_OWNER);
}

#[test]
fn test_sample_price_update_malformed_rejected() {
    let (mut env, w) = Env::with_window(24);
    env.set_time(b(2));
    let addr = Address::new_unique();
    let mut bad = at(1, 100_000_000).bytes();
    bad[0] ^= 1;
    env.set(addr, RECEIVER, bad);
    expect_code(&env.sample_at(&w, &addr), code::ORACLE_WRONG_DISCRIMINATOR);
    let mut short = at(1, 100_000_000).bytes();
    short.truncate(100);
    env.set(addr, RECEIVER, short);
    expect_code(&env.sample_at(&w, &addr), code::ORACLE_TRUNCATED);
    let mut tag = at(1, 100_000_000).bytes();
    tag[40] = 7;
    env.set(addr, RECEIVER, tag);
    expect_code(&env.sample_at(&w, &addr), code::ORACLE_UNKNOWN_LEVEL);
}

#[test]
fn test_sample_real_mainnet_fixture_account() {
    // The committed SOL/USD push feed account, sampled into a window built around its
    // publish time.
    let data = include_bytes!("../../pyth-lite/fixtures/sol_usd.data.bin").to_vec();
    let u = pyth_lite::PriceUpdate::decode(&data).unwrap();
    let publish = u.message().publish_time().get();
    let prev = u.message().prev_publish_time().get();
    assert!(prev < publish);
    let mut env = Env::new();
    env.set_time(publish - 10 * i64::from(H));
    env.init_config().unwrap();
    env.set_feed(feed_args(SOL)).unwrap();
    // A 60 s grid whose b_1 is the publish second.
    let start = publish - 60;
    let end = start + 120;
    env.create_window(SOL, start, end, 60).unwrap();
    let w = window_pda(SOL, start, end, 60);
    let addr = Address::new_unique();
    env.set(addr, RECEIVER, data);
    env.set_time(publish + 5);
    let meta = env.sample_at(&w, &addr).unwrap();
    let r: SampleResult = bytemuck::pod_read_unaligned(&meta.return_data.data);
    assert_eq!((r.first, r.last), (1, 1));
    assert_eq!(r.price, u.message().price());
}

#[test]
fn test_sample_replay_rejected() {
    let (mut env, w) = Env::with_window(24);
    env.set_time(b(2));
    let p = at(1, 100_000_000);
    env.sample(&w, &p).unwrap();
    expect_code(&env.sample(&w, &p), code::BUCKET_ALREADY_SAMPLED);
}

#[test]
fn test_sample_forged_window_rejected() {
    let (mut env, w) = Env::with_window(24);
    env.set_time(b(2));
    let real = env.account(&w);
    // The window's bytes under another owner.
    let forged = Address::new_unique();
    env.set(forged, Address::new_unique(), real.data.clone());
    expect_code(
        &env.sample(&forged, &at(1, 100_000_000)),
        code::INVALID_ACCOUNT_OWNER,
    );
    // A program-owned account that is not a window (the feed config).
    expect_code(
        &env.sample(&feed_pda(SOL), &at(1, 100_000_000)),
        code::CORE_INVALID_ACCOUNT_DATA,
    );
    // A window whose header was tampered with.
    let mut data = real.data.clone();
    data[108] = 9; // filled counter
    let tampered = Address::new_unique();
    env.set(tampered, PROGRAM_ID, data);
    expect_code(
        &env.sample(&tampered, &at(1, 100_000_000)),
        code::CORE_INVALID_ACCOUNT_DATA,
    );
}

#[test]
fn test_sample_window_not_writable_rejected() {
    let (mut env, w) = Env::with_window(24);
    env.set_time(b(2));
    let addr = Address::new_unique();
    env.set(addr, RECEIVER, at(1, 100_000_000).bytes());
    let mut ix = ix_sample(&w, &addr);
    ix.accounts[0].is_writable = false;
    let p = env.payer.insecure_clone();
    expect_code(&env.send(&[ix], &[&p]), code::NOT_WRITABLE);
}

#[test]
fn test_window_bound_to_its_feed_config_copy() {
    // After creation, the admin moves the SOL feed to another receiver, a 1 h guard and a 1%
    // confidence bound. The window keeps sampling under the parameters it copied.
    let (mut env, w) = Env::with_window(24);
    let mut a = feed_args(SOL);
    a.receiver_program = [0x77; 32];
    a.tolerance = H;
    a.max_conf = 100;
    env.set_feed(a).unwrap();
    env.set_time(b(10));
    // conf 5% (above the new 1%, within the copied 10%), owned by the original receiver.
    env.sample(
        &w,
        &PriceUpdate {
            conf: 5_000_000,
            ..at(0, 100_000_000)
        },
    )
    .unwrap();
    // A 3 h halt: outside the new 1 h guard, inside the copied 6 h one.
    let meta = env
        .sample(&w, &PriceUpdate::sol(100_000_000, b(4) + 5, b(1) - 10))
        .unwrap();
    let r: SampleResult = bytemuck::pod_read_unaligned(&meta.return_data.data);
    assert_eq!((r.first, r.last), (1, 4));
}

// ---- finalize --------------------------------------------------------------------------

#[test]
fn test_finalize_rules() {
    let (mut env, w) = Env::with_window(2);
    env.set_time(b(1));
    expect_code(&env.finalize(&w), code::FINALIZE_TOO_EARLY);
    env.sample(&w, &at(0, 100_000_000)).unwrap();
    env.sample(&w, &at(2, 100_000_000)).unwrap();
    let deadline = b(2) + i64::from(BACKFILL);
    env.set_time(deadline);
    expect_code(&env.finalize(&w), code::FINALIZE_TOO_EARLY);
    env.set_time(deadline + 1);
    env.finalize(&w).unwrap();
    assert_eq!(state(&env, &w), WindowState::Degraded);
    expect_code(&env.finalize(&w), code::WINDOW_FINAL);
    expect_code(&env.sample(&w, &at(1, 100_000_000)), code::WINDOW_FINAL);
}

#[test]
fn test_finalize_complete_window_immediately() {
    let (mut env, w) = Env::with_window(2);
    env.set_time(b(2) + 2);
    for k in 0..=2 {
        env.sample(&w, &at(k, 100_000_000 + i64::from(k))).unwrap();
    }
    env.finalize(&w).unwrap();
    assert_eq!(state(&env, &w), WindowState::Finalized);
}

#[test]
fn test_finalize_forged_window_rejected() {
    let (mut env, w) = Env::with_window(2);
    let real = env.account(&w);
    let forged = Address::new_unique();
    env.set(forged, Address::new_unique(), real.data.clone());
    expect_code(&env.finalize(&forged), code::INVALID_ACCOUNT_OWNER);
    let mut ix = ix_finalize(&w);
    ix.accounts[0].is_writable = false;
    let p = env.payer.insecure_clone();
    expect_code(&env.send(&[ix], &[&p]), code::NOT_WRITABLE);
}

// ---- replay ----------------------------------------------------------------------------

/// A synthetic SOL-like tape for 25 hourly boundaries, built from the canonical-print
/// definition (docs/PLAN.md 4.1). Synthetic because Hermes historical prices need an API key
/// (verified facts, 2026-10-29); the replay from real fixtures is milestone 6.
///
/// - Prices near 150 USD (exponent −8) on a splitmix64 walk.
/// - The canonical print of each boundary is published 0 to 40 s after it; its
///   `prev_publish_time` is the previous successful aggregate.
/// - A 2 h halt over b_7..=b_8: the reopening print fills b_7..=b_9.
/// - Noise that must be rejected (`NotOnGrid`): an intermediate aggregate between each pair
///   of boundaries, a same-second second aggregate after each canonical print, and a
///   failed-slot copy (`prev_publish_time == publish_time`) of each.
fn synthetic_tape() -> (Vec<PriceUpdate>, Vec<PriceUpdate>) {
    let mut seed = 0x5EED_0000_0000_0003u64;
    let mut next = || {
        seed = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = seed;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    let mut price = 15_000_000_000i64;
    let mut last = b(0) - 7; // the last successful aggregate before b_0
    let (mut canonical, mut noise) = (Vec::new(), Vec::new());
    for k in 0..=24u32 {
        if k == 7 || k == 8 {
            continue; // halted: no aggregate crosses b_7 or b_8
        }
        price += (next() % 400_000_000) as i64 - 200_000_000;
        let publish = b(k) + (next() % 41) as i64;
        canonical.push(PriceUpdate::sol(price, publish, last));
        // Same second, later slot: prev == publish.
        noise.push(PriceUpdate::sol(price + 1, publish, publish));
        // Failed slot after it: last trading price, prev == publish.
        noise.push(PriceUpdate::sol(price, publish, publish));
        // One intermediate aggregate before the next boundary.
        let mid = b(k) + 60 + (next() % 3_000) as i64;
        noise.push(PriceUpdate::sol(price - 1, mid, publish));
        last = mid;
    }
    (canonical, noise)
}

#[test]
fn test_replay_synthetic_24_samples_matches_off_chain() {
    let (mut env, w) = Env::with_window(24);
    let (canonical, noise) = synthetic_tape();
    // Off-chain: the same prints through varswap-core with the same kernel.
    let spec = WindowSpec::try_new(
        pyth_lite::FeedId::new(SOL),
        UnixTimestamp::new(b(0)),
        UnixTimestamp::new(b(24)),
        Seconds::new(H),
        RiskParams {
            tolerance: Seconds::new(TOLERANCE),
            max_conf: Bps::new(MAX_CONF),
            backfill_deadline: Seconds::new(BACKFILL),
            receiver_program: pyth_lite::RECEIVER_PROGRAM_ID,
            exponent: -8,
        },
    )
    .unwrap();
    let mut off = vec![0u8; window_size(25).unwrap()];
    let mut wo = Window::init(&mut off, spec, 0).unwrap();
    let now = UnixTimestamp::new(b(24) + 60);
    env.set_time(now.get());
    let print = |u: &PriceUpdate| {
        Print::try_new(
            pyth_lite::FeedId::new(SOL),
            NonZeroU64::new(u.price as u64).unwrap(),
            u.exponent,
            UnixTimestamp::new(u.publish_time),
            UnixTimestamp::new(u.prev_publish_time),
        )
        .unwrap()
    };
    // Interleave noise with the canonical prints, canonical ones in a scrambled order.
    let mut order: Vec<usize> = (0..canonical.len()).collect();
    order.sort_by_key(|i| (i * 7) % canonical.len());
    for (j, &i) in order.iter().enumerate() {
        for u in noise.iter().skip(3 * j).take(3) {
            expect_code(&env.sample(&w, u), code::NOT_ON_GRID);
            assert_eq!(
                wo.sample(&print(u), now, &LnKernel),
                Err(varswap_core::CoreError::NotOnGrid)
            );
        }
        let u = &canonical[i];
        env.sample(&w, u).unwrap();
        wo.sample(&print(u), now, &LnKernel).unwrap();
    }
    assert_eq!(canonical.len(), 23);
    env.finalize(&w).unwrap();
    assert_eq!(wo.finalize(now, &LnKernel).unwrap(), WindowState::Finalized);
    let mut d = load(&env, &w);
    let on = Window::load(&mut d).unwrap();
    assert_eq!(on.state().unwrap(), WindowState::Finalized);
    assert_eq!(on.filled(), 25);
    assert_eq!(on.price(8).unwrap(), on.price(9).unwrap());
    assert_eq!(on.sum_sq(), wo.sum_sq());
    assert_eq!(on.realized_var().unwrap(), wo.realized_var().unwrap());
    assert!(on.sum_sq().raw() > 0);
    // Everything but the header bump is byte-identical (the off-chain copy used bump 0).
    assert_eq!(d[3..], off[3..]);
}

#[test]
fn test_sample_event_logged() {
    let (mut env, w) = Env::with_window(24);
    env.set_time(b(2));
    env.sample(&w, &at(0, 100_000_000)).unwrap();
    let meta = env.sample(&w, &at(1, 100_000_300)).unwrap();
    let line = meta
        .logs
        .iter()
        .find(|l| l.contains("varswap:sample"))
        .expect("no sample event");
    assert!(
        line.ends_with(&format!(
            "varswap:sample first=1 last=1 publish_time={} price=100000300 expo=-8",
            b(1) + 1
        )),
        "{line}"
    );
}

#[test]
fn test_sample_exponent_mismatch_rejected() {
    // The SOL feed config pins −8; a print at −7 is the same feed after a Pyth re-init.
    let (mut env, w) = Env::with_window(24);
    env.set_time(b(2));
    let u = PriceUpdate {
        exponent: -7,
        ..at(1, 10_000_000)
    };
    expect_code(&env.sample(&w, &u), code::EXPONENT_MISMATCH);
    // The window keeps its copy even if the admin re-pins the feed afterwards.
    let mut a = feed_args(SOL);
    a.exponent = -7;
    env.set_feed(a).unwrap();
    expect_code(&env.sample(&w, &u), code::EXPONENT_MISMATCH);
    env.sample(&w, &at(1, 100_000_000)).unwrap();
}
