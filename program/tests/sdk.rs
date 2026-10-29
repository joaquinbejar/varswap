//! `varswap-sdk` against the program in LiteSVM:
//! - every builder produces exactly the instruction the test harness restates independently
//!   (accounts, signer and writable flags, data), so the two encodings check each other;
//! - a full lifecycle (config, feed config, window, swap, sampling, finalize, settle, both
//!   claims, close) where every varswap instruction comes from the SDK and every account is
//!   read back with its decoders. Only token setup and Pyth price-update fixtures are done by
//!   hand. Run `make program` first.

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

use common::*;
use solana_address::Address;
use solana_keypair::Keypair;
use solana_signer::Signer;
use varswap_core::WindowState;
use varswap_sdk::decode::{self, DecodeError};
use varswap_sdk::ix::{self, FeedParams, OpenSwap};
use varswap_sdk::pda::{self, WindowKey};

const START: i64 = T0 + H as i64;

fn sdk_feed(a: &varswap_program::layout::SetFeedConfigArgs) -> FeedParams {
    FeedParams {
        feed: a.feed,
        receiver_program: Address::new_from_array(a.receiver_program),
        min_interval: a.min_interval,
        tolerance: a.tolerance,
        backfill_deadline: a.backfill_deadline,
        max_buckets: a.max_buckets,
        max_conf: a.max_conf,
        exponent: a.exponent,
    }
}

fn key(n: u32) -> WindowKey {
    WindowKey {
        feed: SOL,
        start: START,
        end: START + i64::from(H) * i64::from(n),
        interval: H,
    }
}

#[test]
fn test_sdk_instructions_equal_the_independent_encodings() {
    let env = Env::new();
    let (admin, payer, mint) = (Address::new_unique(), Address::new_unique(), env.usdc);
    let k = key(24);
    let window = window_pda(SOL, k.start, k.end, k.interval);
    assert_eq!(pda::window(&PROGRAM_ID, &k).0, window);
    assert_eq!(pda::config(&PROGRAM_ID).0, config_pda());
    assert_eq!(pda::feed_config(&PROGRAM_ID, &SOL).0, feed_pda(SOL));
    assert_eq!(pda::program_data(&PROGRAM_ID).0, Env::program_data());
    let fa = feed_args(SOL);
    let pu = Address::new_unique();
    let open = Open {
        window,
        long: Address::new_unique(),
        short: Address::new_unique(),
        payer,
        long_src: Address::new_unique(),
        short_src: Address::new_unique(),
        mint,
        notional: 1_000_000_007,
        strike: ONE_VAR / 100 + 3,
        cap: ONE_VAR / 16 + 5,
        index: 0x0102_0304_0506_0708,
    };
    let sdk_open = OpenSwap {
        window: open.window,
        long: open.long,
        short: open.short,
        payer: open.payer,
        long_src: open.long_src,
        short_src: open.short_src,
        mint: open.mint,
        notional: open.notional,
        strike_var: open.strike,
        cap_var: open.cap,
        index: open.index,
    };
    let swap = open.swap();
    assert_eq!(
        pda::swap(&PROGRAM_ID, &window, &open.long, &open.short, open.index).0,
        swap
    );
    assert_eq!(pda::vault(&PROGRAM_ID, &swap).0, vault_pda(&swap));
    let (dest, receiver, receiver_token) = (
        Address::new_unique(),
        Address::new_unique(),
        Address::new_unique(),
    );
    let pairs = vec![
        (
            "init_config",
            ix::init_config(&PROGRAM_ID, &admin, &payer, &mint),
            ix_init_config(&admin, &payer, &mint),
        ),
        (
            "set_feed_config",
            ix::set_feed_config(&PROGRAM_ID, &admin, &payer, &sdk_feed(&fa)),
            ix_set_feed_config(&admin, &payer, &fa),
        ),
        (
            "create_window",
            ix::create_window(&PROGRAM_ID, &payer, &k),
            ix_create_window(&payer, SOL, k.start, k.end, k.interval),
        ),
        (
            "sample",
            ix::sample(&PROGRAM_ID, &window, &pu),
            ix_sample(&window, &pu),
        ),
        (
            "finalize",
            ix::finalize(&PROGRAM_ID, &window),
            ix_finalize(&window),
        ),
        (
            "open_swap",
            ix::open_swap(&PROGRAM_ID, &sdk_open),
            open.ix(),
        ),
        (
            "settle_swap",
            ix::settle_swap(&PROGRAM_ID, &window, &swap),
            ix_settle(&window, &swap),
        ),
        (
            "claim_long",
            ix::claim_long(&PROGRAM_ID, &swap, &dest, &mint),
            ix_claim(true, &swap, &dest, &mint),
        ),
        (
            "claim_short",
            ix::claim_short(&PROGRAM_ID, &swap, &dest, &mint),
            ix_claim(false, &swap, &dest, &mint),
        ),
        (
            "close_swap",
            ix::close_swap(&PROGRAM_ID, &swap, &receiver, &receiver_token, &mint),
            ix_close(&swap, &receiver, &receiver_token, &mint),
        ),
    ];
    for (name, sdk, harness) in pairs {
        assert_eq!(sdk, harness, "{name}");
    }
    // The payloads also match the program's own Pod layouts byte for byte.
    assert_eq!(sdk_feed(&fa).encode(), bytemuck::bytes_of(&fa));
}

#[test]
fn test_full_lifecycle_through_the_sdk() {
    let mut env = Env::new();
    let (admin, payer) = (env.admin.insecure_clone(), env.payer.insecure_clone());
    let usdc = env.usdc;

    env.send(
        &[ix::init_config(
            &PROGRAM_ID,
            &admin.pubkey(),
            &payer.pubkey(),
            &usdc,
        )],
        &[&payer, &admin],
    )
    .unwrap();
    let c = env.account(&pda::config(&PROGRAM_ID).0);
    let cfg = decode::config(&PROGRAM_ID, &c.owner, &c.data).unwrap();
    assert_eq!(cfg.collateral_mint, usdc.to_bytes());
    assert_eq!(
        decode::feed_config(&PROGRAM_ID, &c.owner, &c.data),
        Err(DecodeError::WrongSize)
    );

    let fp = sdk_feed(&feed_args(SOL));
    env.send(
        &[ix::set_feed_config(
            &PROGRAM_ID,
            &admin.pubkey(),
            &payer.pubkey(),
            &fp,
        )],
        &[&payer, &admin],
    )
    .unwrap();
    let f = env.account(&pda::feed_config(&PROGRAM_ID, &SOL).0);
    assert_eq!(
        decode::feed_config(&PROGRAM_ID, &f.owner, &f.data)
            .unwrap()
            .tolerance,
        TOLERANCE
    );

    let k = key(2);
    env.send(
        &[ix::create_window(&PROGRAM_ID, &payer.pubkey(), &k)],
        &[&payer],
    )
    .unwrap();
    let window = pda::window(&PROGRAM_ID, &k).0;
    let wa = env.account(&window);
    let info = decode::window(&PROGRAM_ID, &wa.owner, &wa.data).unwrap();
    assert_eq!(info.state, WindowState::Pending);
    assert_eq!(
        decode::filled_buckets(&PROGRAM_ID, &wa.owner, &wa.data).unwrap(),
        vec![false; 3]
    );

    let (long, short) = (Keypair::new(), Keypair::new());
    let long_src = env.funded(&usdc, &long.pubkey(), 1_000_000_000_000);
    let short_src = env.funded(&usdc, &short.pubkey(), 1_000_000_000_000);
    let payer_token = env.funded(&usdc, &payer.pubkey(), 0);
    let o = OpenSwap {
        window,
        long: long.pubkey(),
        short: short.pubkey(),
        payer: payer.pubkey(),
        long_src,
        short_src,
        mint: usdc,
        notional: 1_000_000_000,
        strike_var: ONE_VAR / 100,
        cap_var: ONE_VAR / 16,
        index: 7,
    };
    env.send(&[ix::open_swap(&PROGRAM_ID, &o)], &[&payer, &long, &short])
        .unwrap();
    let swap = pda::swap(&PROGRAM_ID, &window, &o.long, &o.short, o.index).0;
    let s = env.account(&swap);
    let sd = decode::swap(&PROGRAM_ID, &s.owner, &s.data).unwrap();
    assert_eq!(
        (sd.long_collateral, sd.short_collateral, sd.state),
        (10_000_000, 52_500_001, 0)
    );
    // A swap account is not a config, and a foreign owner is refused.
    assert_eq!(
        decode::config(&PROGRAM_ID, &s.owner, &s.data),
        Err(DecodeError::WrongSize)
    );
    assert_eq!(
        decode::swap(&PROGRAM_ID, &Address::new_unique(), &s.data),
        Err(DecodeError::WrongOwner)
    );

    // Samples: the price-update accounts are fixtures; the instructions come from the SDK.
    env.set_time(k.end + 5);
    for (i, p) in [10_000_000_000i64, 10_010_000_000, 10_000_000_000]
        .iter()
        .enumerate()
    {
        let b = k.start + i64::from(H) * i as i64;
        let pu = Address::new_unique();
        env.set(pu, RECEIVER, PriceUpdate::sol(*p, b + 1, b - 1).bytes());
        env.send(&[ix::sample(&PROGRAM_ID, &window, &pu)], &[&payer])
            .unwrap();
    }
    env.send(&[ix::finalize(&PROGRAM_ID, &window)], &[&payer])
        .unwrap();
    let wa = env.account(&window);
    let info = decode::window(&PROGRAM_ID, &wa.owner, &wa.data).unwrap();
    assert_eq!(info.state, WindowState::Finalized);
    assert!(info.realized_var.unwrap() > 0);
    assert_eq!(
        decode::filled_buckets(&PROGRAM_ID, &wa.owner, &wa.data).unwrap(),
        vec![true; 3]
    );

    env.send(&[ix::settle_swap(&PROGRAM_ID, &window, &swap)], &[&payer])
        .unwrap();
    let sd = decode::swap(&PROGRAM_ID, &PROGRAM_ID, &env.account(&swap).data).unwrap();
    assert_eq!(sd.state, 1);
    assert_eq!(sd.long_payout + sd.short_payout, 62_500_001);
    env.send(
        &[ix::claim_long(&PROGRAM_ID, &swap, &long_src, &usdc)],
        &[&payer],
    )
    .unwrap();
    env.send(
        &[ix::claim_short(&PROGRAM_ID, &swap, &short_src, &usdc)],
        &[&payer],
    )
    .unwrap();
    assert_eq!(
        env.balance(&long_src) + env.balance(&short_src),
        2_000_000_000_000
    );
    env.send(
        &[ix::close_swap(
            &PROGRAM_ID,
            &swap,
            &payer.pubkey(),
            &payer_token,
            &usdc,
        )],
        &[&payer],
    )
    .unwrap();
    assert!(env.svm.get_account(&swap).is_none_or(|a| a.lamports == 0));
    assert_eq!(pda::ata(&payer.pubkey(), &usdc), payer_token);
}
