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
    config_json, create_window_ix, describe, feed_config_json, init_config_ix, read_config,
    read_feed_config, read_swap, read_window, set_feed_config_ix, submit, window_json, AccountInfo,
    CliError, Rpc,
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
