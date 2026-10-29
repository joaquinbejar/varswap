//! `open_swap`, `settle_swap`, `claim_long` / `claim_short` and `close_swap` in LiteSVM, with
//! a forged-account negative test per rule of `docs/PLAN.md` 3.3, the error-bound gate at the
//! limit and above it, frozen-account claims in both orders, and end-to-end conservation.

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
use solana_instruction::AccountMeta;
use solana_keypair::Keypair;
use solana_signer::Signer;
use varswap_core::{LnKernel, Seconds, Settlement, SwapTerms, Window, WindowState};
use varswap_program::layout::SwapData;

const START: i64 = T0 + H as i64;
const FUNDS: u64 = 1_000_000_000_000;
/// 100.0 and 100.1 USD at exponent −8.
const P100: i64 = 10_000_000_000;
const P1001: i64 = 10_010_000_000;

fn b(k: u32) -> i64 {
    START + i64::from(H) * i64::from(k)
}

struct Fx {
    env: Env,
    long: Keypair,
    short: Keypair,
    open: Open,
}

/// A 2-interval hourly window, both parties funded, terms: 1,000 USDC per unit variance,
/// strike 0.01, cap 0.0625.
fn fixture() -> Fx {
    let (mut env, window) = Env::with_window(2);
    let (long, short) = (Keypair::new(), Keypair::new());
    let usdc = env.usdc;
    let long_src = env.funded(&usdc, &long.pubkey(), FUNDS);
    let short_src = env.funded(&usdc, &short.pubkey(), FUNDS);
    let open = Open {
        window,
        long: long.pubkey(),
        short: short.pubkey(),
        payer: env.payer.pubkey(),
        long_src,
        short_src,
        mint: usdc,
        notional: 1_000_000_000,
        strike: ONE_VAR / 100,
        cap: ONE_VAR / 16, // 0.0625, exact
        index: 0,
    };
    Fx {
        env,
        long,
        short,
        open,
    }
}

fn send_open(fx: &mut Fx, open: &Open) -> Res {
    let (p, l, s) = (
        fx.env.payer.insecure_clone(),
        fx.long.insecure_clone(),
        fx.short.insecure_clone(),
    );
    fx.env.send(&[open.ix()], &[&p, &l, &s])
}

fn swap_data(env: &Env, swap: &Address) -> SwapData {
    bytemuck::pod_read_unaligned(&env.account(swap).data)
}

/// Samples every bucket with `prices` and finalizes after the window ends.
fn finalize_with(fx: &mut Fx, prices: &[i64]) {
    let w = fx.open.window;
    fx.env.set_time(b(prices.len() as u32 - 1) + 5);
    for (k, &p) in prices.iter().enumerate() {
        let k = k as u32;
        fx.env
            .sample(&w, &PriceUpdate::sol(p, b(k) + 1, b(k) - 1))
            .unwrap();
    }
    fx.env.finalize(&w).unwrap();
}

fn degrade(fx: &mut Fx) {
    let w = fx.open.window;
    fx.env.set_time(b(2) + i64::from(BACKFILL) + 1);
    fx.env.finalize(&w).unwrap();
}

fn settle(fx: &mut Fx) -> Res {
    let ix = ix_settle(&fx.open.window, &fx.open.swap());
    let p = fx.env.payer.insecure_clone();
    fx.env.send(&[ix], &[&p])
}

fn claim(fx: &mut Fx, long: bool, dest: &Address) -> Res {
    let ix = ix_claim(long, &fx.open.swap(), dest, &fx.open.mint);
    let p = fx.env.payer.insecure_clone();
    fx.env.send(&[ix], &[&p])
}

/// The payouts the core computes for the window's state.
fn expected_payouts(fx: &Fx) -> (u64, u64) {
    let t = SwapTerms::try_new(
        fx.open.notional,
        fx.open.strike,
        fx.open.cap,
        Seconds::new(H),
    )
    .unwrap();
    let mut data = fx.env.account(&fx.open.window).data;
    let w = Window::load(&mut data).unwrap();
    let s = match w.state().unwrap() {
        WindowState::Finalized => Settlement::Realized(
            LnKernel::conservative_realized_var(
                w.realized_var().unwrap().unwrap(),
                Seconds::new(H),
                fx.open.cap,
                0,
            )
            .unwrap(),
        ),
        _ => Settlement::Unwind,
    };
    t.payouts(s).unwrap()
}

// ---- open_swap -------------------------------------------------------------------------

#[test]
fn test_open_swap_moves_recomputed_collateral() {
    let mut fx = fixture();
    let open = fx.open;
    send_open(&mut fx, &open).unwrap();
    let s = swap_data(&fx.env, &open.swap());
    // 1e9 × 0.01 and 1e9 × 0.0525, rounded up. 0.01 is not exact in Q64.64 (its raw value is
    // floor(2^64 / 100)), so cap − strike is just above 0.0525 and the short's collateral
    // rounds up to 52,500,001: the extra atom is the short's own (docs/PLAN.md 4.4).
    assert_eq!(
        (s.long_collateral, s.short_collateral),
        (10_000_000, 52_500_001)
    );
    assert_eq!(fx.env.balance(&open.vault()), 62_500_001);
    assert_eq!(fx.env.balance(&open.long_src), FUNDS - 10_000_000);
    assert_eq!(fx.env.balance(&open.short_src), FUNDS - 52_500_001);
    assert_eq!(s.window, open.window.to_bytes());
    assert_eq!(s.mint, open.mint.to_bytes());
    assert_eq!(s.token_program, TOKEN.to_bytes());
    assert_eq!(s.payer, open.payer.to_bytes());
    assert_eq!(s.state, 0);
    // The vault is owned by the token program, with the swap PDA as its authority.
    let v = fx.env.account(&open.vault());
    assert_eq!(v.owner, TOKEN);
    assert_eq!(v.data[32..64], open.swap().to_bytes());
}

#[test]
fn test_open_swap_twice_and_second_index() {
    let mut fx = fixture();
    let open = fx.open;
    send_open(&mut fx, &open).unwrap();
    expect_code(&send_open(&mut fx, &open), code::ALREADY_INITIALIZED);
    let second = Open { index: 1, ..open };
    send_open(&mut fx, &second).unwrap();
}

#[test]
fn test_open_swap_after_window_start_rejected() {
    let mut fx = fixture();
    let open = fx.open;
    fx.env.set_time(START);
    expect_code(&send_open(&mut fx, &open), code::WINDOW_STARTED);
}

#[test]
fn test_open_swap_on_final_window_rejected() {
    let mut fx = fixture();
    let open = fx.open;
    degrade(&mut fx);
    expect_code(&send_open(&mut fx, &open), code::WINDOW_STARTED);
}

#[test]
fn test_open_swap_terms_rejections() {
    let mut fx = fixture();
    let open = fx.open;
    let cases = [
        (
            "zero notional",
            Open {
                notional: 0,
                ..open
            },
            code::ZERO_NOTIONAL,
        ),
        (
            "zero strike",
            Open { strike: 0, ..open },
            code::INVALID_STRIKE,
        ),
        (
            "cap at strike",
            Open {
                cap: open.strike,
                ..open
            },
            code::INVALID_CAP,
        ),
        (
            "cap below strike",
            Open {
                cap: open.strike - 1,
                ..open
            },
            code::INVALID_CAP,
        ),
    ];
    for (name, o, want) in cases {
        assert_eq!(err_code(&send_open(&mut fx, &o)), want, "{name}");
    }
}

#[test]
fn test_open_swap_error_gate_at_limit_and_above() {
    let mut fx = fixture();
    let cap = 2 * ONE_VAR;
    let eps = LnKernel::realized_var_error_bound(Seconds::new(H), cap, 0).unwrap();
    let max = u64::try_from((ONE_VAR - 1) / eps).unwrap();
    // Collateral at the largest notional: max × 1.0 on each side.
    let admin = fx.env.admin.insecure_clone();
    let usdc = fx.env.usdc;
    for dest in [fx.open.long_src, fx.open.short_src] {
        litesvm_token::MintTo::new(&mut fx.env.svm, &admin, &usdc, &dest, 2 * max)
            .send()
            .unwrap();
    }
    let at_limit = Open {
        notional: max,
        strike: ONE_VAR,
        cap,
        ..fx.open
    };
    let above = Open {
        notional: max + 1,
        index: 1,
        ..at_limit
    };
    expect_code(
        &send_open(&mut fx, &above),
        code::NOTIONAL_ABOVE_ERROR_BOUND,
    );
    send_open(&mut fx, &at_limit).unwrap();
    let d = swap_data(&fx.env, &at_limit.swap());
    assert_eq!(
        (d.notional, d.long_collateral, d.short_collateral),
        (max, max, max)
    );
}

#[test]
fn test_open_swap_signatures_required() {
    let mut fx = fixture();
    let open = fx.open;
    for (i, name) in [(0usize, "long"), (1, "short")] {
        let mut ix = open.ix();
        ix.accounts[i].is_signer = false;
        let (p, l, s) = (
            fx.env.payer.insecure_clone(),
            fx.long.insecure_clone(),
            fx.short.insecure_clone(),
        );
        let signers: Vec<&Keypair> = match i {
            0 => vec![&p, &s],
            _ => vec![&p, &l],
        };
        assert_eq!(
            err_code(&fx.env.send(&[ix], &signers)),
            code::MISSING_SIGNATURE,
            "{name}"
        );
    }
}

type ForgedCase = (
    &'static str,
    Open,
    Box<dyn Fn(&mut solana_instruction::Instruction)>,
    u32,
);

#[test]
fn test_open_swap_forged_accounts_rejected() {
    let mut fx = fixture();
    let open = fx.open;
    let other_mint = fx.env.new_mint();
    let stranger = Keypair::new();
    let usdc = fx.env.usdc;
    let strangers = fx.env.funded(&usdc, &stranger.pubkey(), FUNDS);
    let long_other_mint = fx.env.funded(&other_mint, &fx.long.pubkey(), FUNDS);
    let window_copy = Address::new_unique();
    let wdata = fx.env.account(&open.window).data;
    fx.env.set(window_copy, Address::new_unique(), wdata);
    let cases: Vec<ForgedCase> = vec![
        (
            "other mint",
            Open {
                mint: other_mint,
                ..open
            },
            Box::new(|_| {}),
            code::INVALID_MINT,
        ),
        (
            "long source of a stranger",
            Open {
                long_src: strangers,
                ..open
            },
            Box::new(|_| {}),
            code::INVALID_TOKEN_ACCOUNT,
        ),
        (
            "short source of a stranger",
            Open {
                short_src: strangers,
                ..open
            },
            Box::new(|_| {}),
            code::INVALID_TOKEN_ACCOUNT,
        ),
        (
            "long source of another mint",
            Open {
                long_src: long_other_mint,
                ..open
            },
            Box::new(|_| {}),
            code::INVALID_TOKEN_ACCOUNT,
        ),
        (
            "window of another program",
            Open {
                window: window_copy,
                ..open
            },
            Box::new(|_| {}),
            code::INVALID_ACCOUNT_OWNER,
        ),
        (
            "wrong swap PDA",
            open,
            Box::new(|ix| ix.accounts[5] = AccountMeta::new(Address::new_unique(), false)),
            code::INVALID_PDA,
        ),
        (
            "wrong vault PDA",
            open,
            Box::new(|ix| ix.accounts[6] = AccountMeta::new(Address::new_unique(), false)),
            code::INVALID_VAULT,
        ),
        (
            "wrong token program",
            open,
            Box::new(|ix| {
                ix.accounts[10] = AccountMeta::new_readonly(Address::new_unique(), false)
            }),
            code::INVALID_TOKEN_PROGRAM,
        ),
        (
            "wrong system program",
            open,
            Box::new(|ix| {
                ix.accounts[11] = AccountMeta::new_readonly(Address::new_unique(), false)
            }),
            code::INVALID_SYSTEM_PROGRAM,
        ),
        (
            "forged config",
            open,
            Box::new(|ix| ix.accounts[3] = AccountMeta::new_readonly(feed_pda(SOL), false)),
            code::INVALID_ACCOUNT_DATA,
        ),
    ];
    for (name, o, m, want) in cases {
        let mut ix = o.ix();
        m(&mut ix);
        let (p, l, s) = (
            fx.env.payer.insecure_clone(),
            fx.long.insecure_clone(),
            fx.short.insecure_clone(),
        );
        assert_eq!(err_code(&fx.env.send(&[ix], &[&p, &l, &s])), want, "{name}");
    }
}

#[test]
fn test_open_swap_insufficient_funds_fails() {
    let mut fx = fixture();
    // A second long token account (not the ATA) holding a single atom.
    let mint = fx.open.mint;
    let admin = fx.env.admin.insecure_clone();
    let poor = litesvm_token::CreateAccount::new(&mut fx.env.svm, &admin, &mint)
        .owner(&fx.long.pubkey())
        .send()
        .unwrap();
    let open = Open {
        long_src: poor,
        ..fx.open
    };
    expect_fail(&send_open(&mut fx, &open));
    assert!(fx
        .env
        .svm
        .get_account(&open.swap())
        .is_none_or(|a| a.data.is_empty()));
}

// ---- settle_swap -----------------------------------------------------------------------

#[test]
fn test_settle_rules() {
    let mut fx = fixture();
    let open = fx.open;
    send_open(&mut fx, &open).unwrap();
    expect_code(&settle(&mut fx), code::WINDOW_NOT_FINAL);
    finalize_with(&mut fx, &[P100, P1001, P100]);
    // Another window.
    let ix = ix_settle(&feed_pda(SOL), &open.swap());
    let p = fx.env.payer.insecure_clone();
    expect_code(&fx.env.send(&[ix], &[&p]), code::WRONG_WINDOW);
    settle(&mut fx).unwrap();
    let s = swap_data(&fx.env, &open.swap());
    assert_eq!(s.state, 1);
    assert_eq!((s.long_payout, s.short_payout), expected_payouts(&fx));
    assert_eq!(s.long_payout + s.short_payout, 62_500_001);
    expect_code(&settle(&mut fx), code::SWAP_ALREADY_SETTLED);
}

#[test]
fn test_settle_degraded_unwinds() {
    let mut fx = fixture();
    let open = fx.open;
    send_open(&mut fx, &open).unwrap();
    degrade(&mut fx);
    settle(&mut fx).unwrap();
    let s = swap_data(&fx.env, &open.swap());
    assert_eq!((s.long_payout, s.short_payout), (10_000_000, 52_500_001));
}

#[test]
fn test_settle_forged_swap_rejected() {
    let mut fx = fixture();
    let open = fx.open;
    send_open(&mut fx, &open).unwrap();
    finalize_with(&mut fx, &[P100, P1001, P100]);
    let real = fx.env.account(&open.swap()).data;
    let forged = Address::new_unique();
    fx.env.set(forged, Address::new_unique(), real.clone());
    let p = fx.env.payer.insecure_clone();
    let ix = ix_settle(&open.window, &forged);
    expect_code(&fx.env.send(&[ix], &[&p]), code::INVALID_ACCOUNT_OWNER);
    // A program account that is not a swap (the window itself).
    let ix = ix_settle(&open.window, &open.window);
    expect_fail(&fx.env.send(&[ix], &[&p]));
}

// ---- claims and close -------------------------------------------------------------------

fn settled(prices: &[i64]) -> Fx {
    let mut fx = fixture();
    let open = fx.open;
    send_open(&mut fx, &open).unwrap();
    finalize_with(&mut fx, prices);
    settle(&mut fx).unwrap();
    fx
}

#[test]
fn test_claims_both_orders_conserve_the_vault() {
    for long_first in [true, false] {
        let mut fx = settled(&[P100, P1001, P100]);
        let open = fx.open;
        let s = swap_data(&fx.env, &open.swap());
        let (l0, s0) = (
            fx.env.balance(&open.long_src),
            fx.env.balance(&open.short_src),
        );
        let order = if long_first {
            [true, false]
        } else {
            [false, true]
        };
        for side in order {
            let dest = if side { open.long_src } else { open.short_src };
            claim(&mut fx, side, &dest).unwrap();
            expect_code(&claim(&mut fx, side, &dest), code::ALREADY_CLAIMED);
        }
        assert_eq!(fx.env.balance(&open.long_src) - l0, s.long_payout);
        assert_eq!(fx.env.balance(&open.short_src) - s0, s.short_payout);
        assert_eq!(fx.env.balance(&open.vault()), 0);
        // Conservation over the whole life: what both sides hold now equals what they had.
        assert_eq!(
            fx.env.balance(&open.long_src) + fx.env.balance(&open.short_src),
            2 * FUNDS
        );
    }
}

#[test]
fn test_frozen_side_does_not_block_the_other() {
    for frozen_long in [true, false] {
        let mut fx = settled(&[P100, P1001, P100]);
        let open = fx.open;
        let (frozen, other) = if frozen_long {
            (open.long_src, open.short_src)
        } else {
            (open.short_src, open.long_src)
        };
        fx.env.freeze(&frozen);
        let s = swap_data(&fx.env, &open.swap());
        let frozen_amount = if frozen_long {
            s.long_payout
        } else {
            s.short_payout
        };
        let r = claim(&mut fx, frozen_long, &frozen);
        if frozen_amount > 0 {
            expect_fail(&r);
        }
        claim(&mut fx, !frozen_long, &other).unwrap();
        assert_eq!(
            fx.env.balance(&open.vault()),
            if frozen_amount > 0 { frozen_amount } else { 0 }
        );
    }
}

#[test]
fn test_claim_rules_and_forged_accounts() {
    let mut fx = fixture();
    let open = fx.open;
    send_open(&mut fx, &open).unwrap();
    expect_code(
        &claim(&mut fx, true, &open.long_src),
        code::SWAP_NOT_SETTLED,
    );
    finalize_with(&mut fx, &[P100, P1001, P100]);
    settle(&mut fx).unwrap();
    // The other side's account, a stranger's, another mint.
    expect_code(
        &claim(&mut fx, true, &open.short_src),
        code::INVALID_TOKEN_ACCOUNT,
    );
    let stranger = Keypair::new();
    let usdc = fx.env.usdc;
    let strangers = fx.env.funded(&usdc, &stranger.pubkey(), 0);
    expect_code(
        &claim(&mut fx, false, &strangers),
        code::INVALID_TOKEN_ACCOUNT,
    );
    let p = fx.env.payer.insecure_clone();
    let other_mint = fx.env.new_mint();
    let ix = ix_claim(true, &open.swap(), &open.long_src, &other_mint);
    expect_code(&fx.env.send(&[ix], &[&p]), code::INVALID_MINT);
    // A vault that is not the swap's PDA (another swap's vault).
    let mut ix = ix_claim(true, &open.swap(), &open.long_src, &open.mint);
    ix.accounts[1] = AccountMeta::new(vault_pda(&Address::new_unique()), false);
    expect_code(&fx.env.send(&[ix], &[&p]), code::INVALID_VAULT);
    let mut ix = ix_claim(true, &open.swap(), &open.long_src, &open.mint);
    ix.accounts[4] = AccountMeta::new_readonly(Address::new_unique(), false);
    expect_code(&fx.env.send(&[ix], &[&p]), code::INVALID_TOKEN_PROGRAM);
    claim(&mut fx, true, &open.long_src).unwrap();
}

#[test]
fn test_close_swap_rules_and_refund() {
    let mut fx = settled(&[P100, P1001, P100]);
    let open = fx.open;
    let p = fx.env.payer.insecure_clone();
    expect_code(
        &fx.env.send(&[ix_close(&open.swap(), &open.payer)], &[&p]),
        code::NOT_CLAIMED,
    );
    claim(&mut fx, true, &open.long_src).unwrap();
    expect_code(
        &fx.env.send(&[ix_close(&open.swap(), &open.payer)], &[&p]),
        code::NOT_CLAIMED,
    );
    claim(&mut fx, false, &open.short_src).unwrap();
    let stranger = Address::new_unique();
    expect_code(
        &fx.env.send(&[ix_close(&open.swap(), &stranger)], &[&p]),
        code::WRONG_RECEIVER,
    );
    let rent = fx.env.account(&open.swap()).lamports + fx.env.account(&open.vault()).lamports;
    let before = fx.env.svm.get_balance(&open.payer).unwrap();
    let meta = fx
        .env
        .send(&[ix_close(&open.swap(), &open.payer)], &[&p])
        .unwrap();
    let after = fx.env.svm.get_balance(&open.payer).unwrap();
    assert_eq!(after + meta.fee, before + rent);
    assert!(fx
        .env
        .svm
        .get_account(&open.swap())
        .is_none_or(|a| a.lamports == 0));
    assert!(fx
        .env
        .svm
        .get_account(&open.vault())
        .is_none_or(|a| a.lamports == 0));
}

#[test]
fn test_close_with_frozen_side_stays_locked() {
    let mut fx = settled(&[P100, P1001, P100]);
    let open = fx.open;
    let s = swap_data(&fx.env, &open.swap());
    assert!(s.long_payout > 0 && s.short_payout > 0);
    fx.env.freeze(&open.long_src);
    expect_fail(&claim(&mut fx, true, &open.long_src));
    claim(&mut fx, false, &open.short_src).unwrap();
    let p = fx.env.payer.insecure_clone();
    expect_code(
        &fx.env.send(&[ix_close(&open.swap(), &open.payer)], &[&p]),
        code::NOT_CLAIMED,
    );
}

#[test]
fn test_full_cycle_payouts_match_core_across_paths() {
    // Calm, volatile and above-cap paths, each settled on V_cons and fully claimed.
    for prices in [
        [P100, P100, P100],
        [P100, P1001, P100],
        [P100, 10_300_000_000, P100],
        [P100, 13_000_000_000, P100],
    ] {
        let mut fx = settled(&prices);
        let open = fx.open;
        let s = swap_data(&fx.env, &open.swap());
        assert_eq!(
            (s.long_payout, s.short_payout),
            expected_payouts(&fx),
            "{prices:?}"
        );
        assert_eq!(
            s.long_payout + s.short_payout,
            s.long_collateral + s.short_collateral
        );
        claim(&mut fx, true, &open.long_src).unwrap();
        claim(&mut fx, false, &open.short_src).unwrap();
        assert_eq!(fx.env.balance(&open.vault()), 0);
    }
}
