//! `varswap-cli` against the program in LiteSVM: the admin CLI's commands (the same functions
//! `varswap-admin` calls) run through an in-process [`Rpc`], from `init_config` to a window,
//! and every read decodes what the program wrote. Run `make program` first.

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
use solana_transaction::Transaction;
use varswap_cli::{
    claim_ix, close_swap_ix, config_json, create_window_ix, describe, feed_config_json,
    init_config_ix, read_config, read_feed_config, read_swap, read_window, set_feed_config_ix,
    settle_swap_ix, submit, window_json, AccountInfo, CliError, Rpc, Side,
};
use varswap_sdk::ix::FeedParams;
use varswap_sdk::pda::{self, WindowKey};
use varswap_sdk::PYTH_RECEIVER;

/// [`Rpc`] over the LiteSVM of an [`Env`].
struct Svm<'a>(&'a mut Env);

impl Rpc for Svm<'_> {
    fn latest_blockhash(&mut self) -> Result<solana_hash::Hash, CliError> {
        // A fresh blockhash per transaction, so identical transactions do not collide.
        self.0.svm.expire_blockhash();
        Ok(self.0.svm.latest_blockhash())
    }

    fn account(&mut self, address: &Address) -> Result<Option<AccountInfo>, CliError> {
        Ok(self.0.svm.get_account(address).map(|a| AccountInfo {
            owner: a.owner,
            data: a.data,
        }))
    }

    fn send(&mut self, tx: &Transaction) -> Result<(), CliError> {
        self.0
            .svm
            .send_transaction(tx.clone())
            .map(|_| ())
            .map_err(|e| CliError::Rpc(format!("{:?} {:?}", e.err, e.meta.logs)))
    }
}

fn sol_feed() -> FeedParams {
    FeedParams {
        feed: SOL,
        receiver_program: PYTH_RECEIVER,
        min_interval: 60,
        tolerance: TOLERANCE,
        backfill_deadline: BACKFILL,
        max_buckets: MAX_BUCKETS,
        max_conf: MAX_CONF,
        exponent: -8,
    }
}

#[test]
fn test_cli_commands_run_against_the_program() {
    let mut env = Env::new();
    let admin = env.admin.insecure_clone();
    let usdc = env.usdc;

    // Before init: the config read is a clean not-found.
    assert!(matches!(
        read_config(&mut Svm(&mut env), &PROGRAM_ID),
        Err(CliError::AccountNotFound(_))
    ));
    let ix = init_config_ix(&PROGRAM_ID, &admin.pubkey(), &usdc);
    assert!(describe(&ix).contains(&format!("{} signer", admin.pubkey())));
    submit(&mut Svm(&mut env), &[ix], &admin).unwrap();
    let c = read_config(&mut Svm(&mut env), &PROGRAM_ID).unwrap();
    let j = config_json(&PROGRAM_ID, &c);
    assert_eq!(j["admin"], admin.pubkey().to_string());
    assert_eq!(j["collateral_mint"], usdc.to_string());

    submit(
        &mut Svm(&mut env),
        &[set_feed_config_ix(
            &PROGRAM_ID,
            &admin.pubkey(),
            &sol_feed(),
        )],
        &admin,
    )
    .unwrap();
    let f = read_feed_config(&mut Svm(&mut env), &PROGRAM_ID, &SOL).unwrap();
    let j = feed_config_json(&PROGRAM_ID, &f);
    assert_eq!(j["tolerance_s"], TOLERANCE);
    assert_eq!(j["exponent"], -8);
    assert_eq!(
        j["feed"],
        "ef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d"
    );

    let k = WindowKey {
        feed: SOL,
        start: T0 + i64::from(H),
        end: T0 + 25 * i64::from(H),
        interval: H,
    };
    submit(
        &mut Svm(&mut env),
        &[create_window_ix(&PROGRAM_ID, &admin.pubkey(), &k).unwrap()],
        &admin,
    )
    .unwrap();
    let w = pda::window(&PROGRAM_ID, &k).0;
    let (info, filled) = read_window(&mut Svm(&mut env), &PROGRAM_ID, &w).unwrap();
    let j = window_json(&w, &info, &filled);
    assert_eq!(j["state"], "pending");
    assert_eq!(j["buckets"], 25);
    assert_eq!(j["missing"].as_array().unwrap().len(), 25);
    assert!(j["realized_var"].is_null());

    // A window is not a swap; a program's account read as the wrong kind is a decode error.
    assert!(matches!(
        read_swap(&mut Svm(&mut env), &PROGRAM_ID, &w),
        Err(CliError::Decode { what: "swap", .. })
    ));
}

#[test]
fn test_cli_set_feed_config_by_another_signer_is_rejected() {
    let mut env = Env::new();
    let admin = env.admin.insecure_clone();
    let usdc = env.usdc;
    submit(
        &mut Svm(&mut env),
        &[init_config_ix(&PROGRAM_ID, &admin.pubkey(), &usdc)],
        &admin,
    )
    .unwrap();
    let other = Keypair::new();
    env.svm.airdrop(&other.pubkey(), 1_000_000_000).unwrap();
    let r = submit(
        &mut Svm(&mut env),
        &[set_feed_config_ix(
            &PROGRAM_ID,
            &other.pubkey(),
            &sol_feed(),
        )],
        &other,
    );
    match r {
        Err(CliError::Rpc(m)) => assert!(
            m.contains(&format!("Custom({})", code::NOT_AUTHORITY)),
            "{m}"
        ),
        other => panic!("{other:?}"),
    }
}

/// A degraded window with one open swap (both sides funded through their associated token
/// accounts), the way `docs/runbooks/degraded-window.md` finds it.
fn degraded_swap() -> (Env, Open, Keypair) {
    let (mut env, window) = Env::with_window(2);
    let (long, short) = (Keypair::new(), Keypair::new());
    let usdc = env.usdc;
    let long_src = env.funded(&usdc, &long.pubkey(), 1_000_000_000);
    let short_src = env.funded(&usdc, &short.pubkey(), 1_000_000_000);
    let payer = env.payer.pubkey();
    env.funded(&usdc, &payer, 0);
    let open = Open {
        window,
        long: long.pubkey(),
        short: short.pubkey(),
        payer,
        long_src,
        short_src,
        mint: usdc,
        notional: 1_000_000_000,
        strike: ONE_VAR / 100,
        cap: ONE_VAR / 16,
        index: 0,
    };
    let p = env.payer.insecure_clone();
    env.send(&[open.ix()], &[&p, &long, &short]).unwrap();
    // Nothing sampled: after the backfill deadline the window degrades.
    env.set_time(T0 + i64::from(H) * 3 + i64::from(BACKFILL) + 1);
    env.finalize(&window).unwrap();
    (env, open, p)
}

#[test]
fn test_cli_settles_claims_and_closes_a_degraded_swap() {
    let (mut env, open, payer) = degraded_swap();
    let swap = open.swap();
    let s = read_swap(&mut Svm(&mut env), &PROGRAM_ID, &swap).unwrap();
    let (long_c, short_c) = (s.long_collateral, s.short_collateral);
    let (long_before, short_before) = (env.balance(&open.long_src), env.balance(&open.short_src));

    let ix = settle_swap_ix(&mut Svm(&mut env), &PROGRAM_ID, &swap).unwrap();
    assert!(describe(&ix).contains(&open.window.to_string()));
    submit(&mut Svm(&mut env), &[ix], &payer).unwrap();
    let s = read_swap(&mut Svm(&mut env), &PROGRAM_ID, &swap).unwrap();
    // Degraded: an unwind, each side its own collateral.
    assert_eq!((s.long_payout, s.short_payout), (long_c, short_c));

    // Default destinations: each side's associated token account.
    for side in [Side::Long, Side::Short] {
        let ix = claim_ix(&mut Svm(&mut env), &PROGRAM_ID, &swap, side, None).unwrap();
        submit(&mut Svm(&mut env), &[ix], &payer).unwrap();
    }
    assert_eq!(env.balance(&open.long_src), long_before + long_c);
    assert_eq!(env.balance(&open.short_src), short_before + short_c);

    let ix = close_swap_ix(&mut Svm(&mut env), &PROGRAM_ID, &swap, None).unwrap();
    submit(&mut Svm(&mut env), &[ix], &payer).unwrap();
    assert!(env.svm.get_account(&swap).is_none_or(|a| a.data.is_empty()));
    assert!(env
        .svm
        .get_account(&open.vault())
        .is_none_or(|a| a.data.is_empty()));
}

#[test]
fn test_cli_settle_rejects_a_forged_swap() {
    let (mut env, open, _) = degraded_swap();
    // The window is the program's but not a swap.
    assert!(matches!(
        settle_swap_ix(&mut Svm(&mut env), &PROGRAM_ID, &open.window),
        Err(CliError::Decode { what: "swap", .. })
    ));
    // The swap's exact bytes under another owner.
    let forged = Address::new_unique();
    let data = env.account(&open.swap()).data;
    env.set(forged, SYSTEM, data);
    assert!(matches!(
        settle_swap_ix(&mut Svm(&mut env), &PROGRAM_ID, &forged),
        Err(CliError::Decode { what: "swap", .. })
    ));
}

#[test]
fn test_cli_claim_rejects_forged_destinations() {
    let (mut env, open, payer) = degraded_swap();
    let swap = open.swap();
    let settle = settle_swap_ix(&mut Svm(&mut env), &PROGRAM_ID, &swap).unwrap();
    submit(&mut Svm(&mut env), &[settle], &payer).unwrap();
    let usage = |r: Result<_, CliError>, what: &str| match r {
        Err(CliError::Usage(m)) => assert!(m.contains(what), "{m}"),
        other => panic!("{other:?}"),
    };
    // The short's account for the long's claim.
    usage(
        claim_ix(
            &mut Svm(&mut env),
            &PROGRAM_ID,
            &swap,
            Side::Long,
            Some(&open.short_src),
        ),
        "not owned by",
    );
    // The long's account of another mint.
    let other_mint = env.new_mint();
    let other = env.funded(&other_mint, &open.long, 0);
    usage(
        claim_ix(
            &mut Svm(&mut env),
            &PROGRAM_ID,
            &swap,
            Side::Long,
            Some(&other),
        ),
        "swap's mint",
    );
    // Not a token account at all.
    let stranger = Address::new_unique();
    env.set(stranger, SYSTEM, vec![0; 165]);
    usage(
        claim_ix(
            &mut Svm(&mut env),
            &PROGRAM_ID,
            &swap,
            Side::Short,
            Some(&stranger),
        ),
        "token program",
    );
    // Past the CLI, the program rejects the same forged destination too.
    let raw = varswap_sdk::ix::claim_long(&PROGRAM_ID, &swap, &open.short_src, &open.mint);
    assert!(submit(&mut Svm(&mut env), &[raw], &payer).is_err());
    assert_eq!(
        read_swap(&mut Svm(&mut env), &PROGRAM_ID, &swap)
            .unwrap()
            .long_claimed,
        0
    );
}

#[test]
fn test_cli_close_rejects_a_forged_receiver_token_account() {
    let (mut env, open, payer) = degraded_swap();
    let swap = open.swap();
    let settle = settle_swap_ix(&mut Svm(&mut env), &PROGRAM_ID, &swap).unwrap();
    submit(&mut Svm(&mut env), &[settle], &payer).unwrap();
    for side in [Side::Long, Side::Short] {
        let ix = claim_ix(&mut Svm(&mut env), &PROGRAM_ID, &swap, side, None).unwrap();
        submit(&mut Svm(&mut env), &[ix], &payer).unwrap();
    }
    // The long's token account instead of the payer's.
    match close_swap_ix(&mut Svm(&mut env), &PROGRAM_ID, &swap, Some(&open.long_src)) {
        Err(CliError::Usage(m)) => assert!(m.contains("not owned by"), "{m}"),
        other => panic!("{other:?}"),
    }
    // Past the CLI, the program rejects it too.
    let raw =
        varswap_sdk::ix::close_swap(&PROGRAM_ID, &swap, &open.payer, &open.long_src, &open.mint);
    assert!(submit(&mut Svm(&mut env), &[raw], &payer).is_err());
    assert!(env
        .svm
        .get_account(&swap)
        .is_some_and(|a| !a.data.is_empty()));
}

#[test]
fn test_cli_rejects_a_swap_of_another_token_program() {
    let (mut env, open, _) = degraded_swap();
    let swap = open.swap();
    // The same swap, its token program field rewritten (offset: header 8, then window, long,
    // short and mint, 32 bytes each).
    let mut acc = env.account(&swap);
    acc.data[8 + 4 * 32..8 + 5 * 32].copy_from_slice(&[7; 32]);
    env.set(swap, PROGRAM_ID, acc.data);
    for r in [
        claim_ix(
            &mut Svm(&mut env),
            &PROGRAM_ID,
            &swap,
            Side::Long,
            Some(&open.long_src),
        ),
        close_swap_ix(&mut Svm(&mut env), &PROGRAM_ID, &swap, None),
    ] {
        match r {
            Err(CliError::Usage(m)) => assert!(m.contains("unsupported"), "{m}"),
            other => panic!("{other:?}"),
        }
    }
}
