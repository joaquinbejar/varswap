//! `varswap-keeper` against the program in LiteSVM, with synthetic tapes: a full window
//! sampled by repeated ticks (same-second repeats, failed aggregations, an outage within the
//! guard), idempotence, an outage beyond the guard that degrades, prints the window rejects,
//! another keeper posting first, and a failing chain that must never finalize. Run
//! `make program` first.

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
use varswap_core::{
    window_size, Bps, LnKernel, Print, RiskParams, Seconds, UnixTimestamp, Window, WindowSpec,
    WindowState,
};
use varswap_keeper::{Chain, Keeper, KeeperError, PriceSource, PrintMsg, TapeSource};
use varswap_sdk::{decode, ix};

const START: i64 = T0 + H as i64;

fn b(k: u32) -> i64 {
    START + i64::from(H) * i64::from(k)
}

/// LiteSVM as the keeper's chain: posts a `PriceUpdateV2` fixture owned by the receiver for
/// each print, then `sample` from the SDK.
struct Svm<'a>(&'a mut Env);

impl Chain for Svm<'_> {
    fn now(&self) -> Result<i64, KeeperError> {
        let c: solana_clock::Clock = self.0.svm.get_sysvar();
        Ok(c.unix_timestamp)
    }

    fn account(&self, a: &Address) -> Result<Option<(Address, Vec<u8>)>, KeeperError> {
        Ok(self.0.svm.get_account(a).map(|acc| (acc.owner, acc.data)))
    }

    fn post_sample(
        &mut self,
        window: &Address,
        feed: &[u8; 32],
        m: &PrintMsg,
    ) -> Result<(), KeeperError> {
        let pu = Address::new_unique();
        let u = PriceUpdate {
            partial: None,
            feed: *feed,
            price: m.price,
            conf: m.conf,
            exponent: m.expo,
            publish_time: m.publish_time,
            prev_publish_time: m.prev_publish_time,
        };
        self.0.set(pu, RECEIVER, u.bytes());
        let payer = self.0.payer.insecure_clone();
        self.0
            .send(&[ix::sample(&PROGRAM_ID, window, &pu)], &[&payer])
            .map(|_| ())
            .map_err(|e| KeeperError::Chain(format!("{:?}", e.err)))
    }

    fn finalize(&mut self, window: &Address) -> Result<(), KeeperError> {
        let payer = self.0.payer.insecure_clone();
        self.0
            .send(&[ix::finalize(&PROGRAM_ID, window)], &[&payer])
            .map(|_| ())
            .map_err(|e| KeeperError::Chain(format!("{:?}", e.err)))
    }
}

/// A synthetic SOL/USD tape (exponent −8) from 10 minutes before `b_0` to `until`: successful
/// aggregates 7 to 53 s apart on a deterministic walk around 150 USD, a same-second repeat
/// every 5th step, a failed-slot copy every 7th, and an optional halt `[from, to)` with no
/// prints.
fn tape(until: i64, halt: Option<(i64, i64)>) -> Vec<PrintMsg> {
    let (mut t, mut last, mut px, mut step) = (START - 600, START - 601, 15_000_000_000i64, 0u64);
    let mut out = Vec::new();
    while t < until {
        step += 1;
        if let Some((from, to)) = halt {
            if (from..to).contains(&t) {
                t = to;
                continue;
            }
        }
        px += ((step * 7_919) % 4_001) as i64 * 10_000 - 20_000_000;
        let msg = |price, publish, prev| PrintMsg {
            price,
            conf: 1_000_000,
            expo: -8,
            publish_time: publish,
            prev_publish_time: prev,
        };
        out.push(msg(px, t, last));
        if step % 5 == 0 {
            out.push(msg(px + 1_000, t, t));
        }
        if step % 7 == 0 {
            out.push(msg(px - 1_000, last, last));
        }
        last = t;
        t += 7 + (step * 31 % 47) as i64;
    }
    out
}

/// The window an honest keeper should produce: every boundary's canonical print through
/// `varswap-core` directly.
fn expected(n: u32, t: &TapeSource) -> (u128, u32) {
    let spec = WindowSpec::try_new(
        pyth_lite::FeedId::new(SOL),
        UnixTimestamp::new(b(0)),
        UnixTimestamp::new(b(n)),
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
    let mut data = vec![0u8; window_size(n + 1).unwrap()];
    let mut w = Window::init(&mut data, spec, 0).unwrap();
    let now = UnixTimestamp::new(b(n) + 86_400);
    for k in 0..=n {
        if w.is_filled(k).unwrap() {
            continue;
        }
        if let Some(m) = t
            .canonical_at(&SOL, b(k), i64::from(TOLERANCE), now.get())
            .unwrap()
        {
            let p = Print::try_new(
                pyth_lite::FeedId::new(SOL),
                NonZeroU64::new(m.price as u64).unwrap(),
                m.expo,
                UnixTimestamp::new(m.publish_time),
                UnixTimestamp::new(m.prev_publish_time),
            )
            .unwrap();
            w.sample(&p, now, &LnKernel).unwrap();
        }
    }
    (w.sum_sq().raw(), w.filled())
}

fn window_state(env: &Env, w: &Address) -> (WindowState, Vec<bool>, u128) {
    let a = env.account(w);
    let info = decode::window(&PROGRAM_ID, &a.owner, &a.data).unwrap();
    let filled = decode::filled_buckets(&PROGRAM_ID, &a.owner, &a.data).unwrap();
    let mut d = a.data.clone();
    let sum = Window::load(&mut d).unwrap().sum_sq().raw();
    (info.state, filled, sum)
}

#[test]
fn test_keeper_samples_a_window_tick_by_tick_and_finalizes() {
    // 6 hourly intervals; a 2 h outage over b_2 and b_3 (within the 6 h guard).
    let n = 6;
    let (mut env, w) = Env::with_window(n);
    let tape = TapeSource::new(SOL, tape(b(n) + 3_600, Some((b(2) - 100, b(3) + 1_800))));
    let keeper = Keeper::new(PROGRAM_ID);
    let mut posted = 0;
    // Tick every 20 minutes from before b_0 to after the end.
    let mut t = START - 1_200;
    while t <= b(n) + 1_200 {
        env.set_time(t);
        let r = keeper.tick(&mut Svm(&mut env), &tape, &w).unwrap();
        posted += r.posted;
        if r.finalized.is_some() {
            break;
        }
        t += 1_200;
    }
    let (state, filled, sum) = window_state(&env, &w);
    assert_eq!(state, WindowState::Finalized);
    assert!(filled.iter().all(|f| *f));
    let (want_sum, want_filled) = expected(n, &tape);
    assert_eq!((sum, n + 1), (want_sum, want_filled));
    // One post for each boundary, except the outage's run (b_2 and b_3 share a print).
    assert_eq!(posted, n);
    // Idempotent: nothing left to do.
    let r = keeper.tick(&mut Svm(&mut env), &tape, &w).unwrap();
    assert!(r.done && r.posted == 0);
}

#[test]
fn test_keeper_outage_beyond_guard_degrades_after_deadline() {
    let n = 12;
    let (mut env, w) = Env::with_window(n);
    // An 8 h halt from before b_2: b_2 and b_3 have no print within the 6 h guard.
    let tape = TapeSource::new(SOL, tape(b(n) + 3_600, Some((b(2) - 100, b(10)))));
    let keeper = Keeper::new(PROGRAM_ID);
    env.set_time(b(n) + 600);
    let r = keeper.tick(&mut Svm(&mut env), &tape, &w).unwrap();
    assert_eq!(r.finalized, None);
    let (state, filled, _) = window_state(&env, &w);
    assert_eq!(state, WindowState::Sampling);
    assert!(!filled[2] && !filled[3] && filled[4] && filled[10]);
    // Past the deadline the keeper finalizes, and the window degrades.
    env.set_time(b(n) + i64::from(BACKFILL) + 1);
    let r = keeper.tick(&mut Svm(&mut env), &tape, &w).unwrap();
    assert_eq!((r.posted, r.finalized), (0, Some(WindowState::Degraded)));
}

#[test]
fn test_keeper_skips_prints_the_window_rejects() {
    let n = 3;
    let (mut env, w) = Env::with_window(n);
    let mut msgs = tape(b(n) + 3_600, None);
    // The canonical print of b_1 has a 20% confidence: the program rejects it, so does the
    // keeper, and the bucket stays empty.
    let src = TapeSource::new(SOL, msgs.clone());
    let c1 = src
        .canonical_at(&SOL, b(1), i64::from(TOLERANCE), i64::MAX)
        .unwrap()
        .unwrap();
    for m in &mut msgs {
        if *m == c1 {
            m.conf = (m.price / 5) as u64;
        }
    }
    let tape = TapeSource::new(SOL, msgs);
    env.set_time(b(n) + 600);
    let r = Keeper::new(PROGRAM_ID)
        .tick(&mut Svm(&mut env), &tape, &w)
        .unwrap();
    assert_eq!((r.posted, r.skipped, r.finalized), (n, 1, None));
    let (_, filled, _) = window_state(&env, &w);
    assert_eq!(filled, vec![true, false, true, true]);
}

#[test]
fn test_keeper_buckets_posted_by_someone_else_are_left_alone() {
    let n = 3;
    let (mut env, w) = Env::with_window(n);
    let tape = TapeSource::new(SOL, tape(b(n) + 3_600, None));
    // Another keeper already posted b_0 and b_2.
    env.set_time(b(n) + 600);
    for k in [0, 2] {
        let m = tape
            .canonical_at(&SOL, b(k), i64::from(TOLERANCE), i64::MAX)
            .unwrap()
            .unwrap();
        Svm(&mut env).post_sample(&w, &SOL, &m).unwrap();
    }
    let r = Keeper::new(PROGRAM_ID)
        .tick(&mut Svm(&mut env), &tape, &w)
        .unwrap();
    assert_eq!((r.posted, r.finalized), (2, Some(WindowState::Finalized)));
}

/// A chain whose posts fail: `lie` posts anyway (another keeper won the race), otherwise
/// nothing is posted.
struct Flaky<'a> {
    inner: Svm<'a>,
    lie: bool,
}

impl Chain for Flaky<'_> {
    fn now(&self) -> Result<i64, KeeperError> {
        self.inner.now()
    }

    fn account(&self, a: &Address) -> Result<Option<(Address, Vec<u8>)>, KeeperError> {
        self.inner.account(a)
    }

    fn post_sample(
        &mut self,
        window: &Address,
        feed: &[u8; 32],
        m: &PrintMsg,
    ) -> Result<(), KeeperError> {
        if self.lie {
            self.inner.post_sample(window, feed, m)?;
        }
        Err(KeeperError::Chain("receiver unavailable".into()))
    }

    fn finalize(&mut self, window: &Address) -> Result<(), KeeperError> {
        self.inner.finalize(window)
    }
}

#[test]
fn test_failed_posts_never_finalize() {
    let n = 3;
    let (mut env, w) = Env::with_window(n);
    let tape = TapeSource::new(SOL, tape(b(n) + 3_600, None));
    env.set_time(b(n) + 600);
    // Posts fail and nothing lands: the tick errors and does not finalize.
    let mut flaky = Flaky {
        inner: Svm(&mut env),
        lie: false,
    };
    let e = Keeper::new(PROGRAM_ID)
        .tick(&mut flaky, &tape, &w)
        .unwrap_err();
    assert_eq!(e, KeeperError::Chain("receiver unavailable".into()));
    assert_eq!(window_state(&env, &w).0, WindowState::Pending);
    // Posts "fail" but land (a race lost to another keeper): benign, and the window completes.
    let mut flaky = Flaky {
        inner: Svm(&mut env),
        lie: true,
    };
    let r = Keeper::new(PROGRAM_ID).tick(&mut flaky, &tape, &w).unwrap();
    assert_eq!(
        (r.posted, r.already_filled, r.finalized),
        (0, n + 1, Some(WindowState::Finalized))
    );
}

#[test]
fn test_keeper_ignores_another_feed_and_future_boundaries() {
    let n = 3;
    let (mut env, w) = Env::with_window(n);
    let other = TapeSource::new(XAU, tape(b(n) + 3_600, None));
    env.set_time(b(n) + 600);
    let r = Keeper::new(PROGRAM_ID)
        .tick(&mut Svm(&mut env), &other, &w)
        .unwrap();
    assert_eq!((r.posted, r.finalized), (0, None));
    // Halfway through the window only the boundaries already passed are posted.
    let (mut env, w) = Env::with_window(n);
    let tape = TapeSource::new(SOL, tape(b(n) + 3_600, None));
    env.set_time(b(1) + 600);
    let r = Keeper::new(PROGRAM_ID)
        .tick(&mut Svm(&mut env), &tape, &w)
        .unwrap();
    assert_eq!(r.posted, 2);
    assert_eq!(window_state(&env, &w).1, vec![true, true, false, false]);
}

/// The keeper's eligibility is the program's: for each kind of print, `accept_print` (what the
/// keeper calls) accepts exactly when `sample` succeeds on-chain.
#[test]
fn test_keeper_eligibility_equals_the_program() {
    let n = 3;
    let mk = |price: i64, conf: u64, expo: i32, publish: i64, prev: i64| PrintMsg {
        price,
        conf,
        expo,
        publish_time: publish,
        prev_publish_time: prev,
    };
    let cases = [
        mk(15_000_000_000, 1_000_000, -8, b(1) + 1, b(1) - 1),
        mk(15_000_000_000, 1_500_000_000, -8, b(1) + 1, b(1) - 1),
        mk(15_000_000_000, 1_500_000_001, -8, b(1) + 1, b(1) - 1),
        mk(0, 0, -8, b(1) + 1, b(1) - 1),
        mk(-5, 0, -8, b(1) + 1, b(1) - 1),
        mk(15_000_000_000, 0, -7, b(1) + 1, b(1) - 1),
        mk(15_000_000_000, 0, -200, b(1) + 1, b(1) - 1),
        mk(15_000_000_000, 0, -8, b(1) + 1, 0),
        mk(15_000_000_000, 0, -8, b(1) + 9, b(1) + 9),
        mk(15_000_000_000, 0, -8, b(1) + 90, b(1) + 9),
        // An outage run within the guard, and one past it.
        mk(15_000_000_000, 0, -8, b(3) + 5, b(1) - 1),
        mk(
            15_000_000_000,
            0,
            -8,
            b(1) + i64::from(TOLERANCE) + 1,
            b(1) - 1,
        ),
    ];
    let mut accepted = 0;
    for (i, m) in cases.iter().enumerate() {
        let (mut env, w) = Env::with_window(n);
        env.set_time(b(n) + 600);
        let a = env.account(&w);
        let spec = decode::window(&PROGRAM_ID, &a.owner, &a.data).unwrap().spec;
        let keeper_says = varswap_core::accept_print(
            &spec,
            &varswap_core::RawPrint {
                feed: spec.feed(),
                price: m.price,
                conf: m.conf,
                exponent: m.expo,
                publish_time: UnixTimestamp::new(m.publish_time),
                prev_publish_time: UnixTimestamp::new(m.prev_publish_time),
            },
        )
        .is_ok();
        let program_says = Svm(&mut env).post_sample(&w, &SOL, m).is_ok();
        assert_eq!(keeper_says, program_says, "case {i}: {m:?}");
        accepted += usize::from(program_says);
    }
    assert_eq!(accepted, 4);
}
