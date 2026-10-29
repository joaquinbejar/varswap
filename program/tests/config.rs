//! `init_config` (the `ProgramData` gate) and `set_feed_config`, with forged accounts.

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
use varswap_program::layout::{ConfigData, FeedConfigData};

// ---- init_config -----------------------------------------------------------------------

#[test]
fn test_init_config_by_upgrade_authority_ok() {
    let mut env = Env::new();
    env.init_config().unwrap();
    let acc = env.account(&config_pda());
    assert_eq!(acc.owner, PROGRAM_ID);
    let c: ConfigData = bytemuck::pod_read_unaligned(&acc.data);
    assert_eq!(c.admin, env.admin.pubkey().to_bytes());
    assert_eq!(c.hdr.tag, 2);
}

#[test]
fn test_init_config_twice_rejected() {
    let mut env = Env::new();
    env.init_config().unwrap();
    expect_code(&env.init_config(), code::ALREADY_INITIALIZED);
}

#[test]
fn test_init_config_front_run_by_other_signer_rejected() {
    let mut env = Env::new();
    let attacker = Keypair::new();
    env.svm.airdrop(&attacker.pubkey(), 1_000_000_000).unwrap();
    let ix = ix_init_config(&attacker.pubkey(), &attacker.pubkey());
    expect_code(&env.send(&[ix], &[&attacker]), code::NOT_AUTHORITY);
}

#[test]
fn test_init_config_program_data_other_authority_rejected() {
    let mut env = Env::new();
    env.set_upgrade_authority(Some(Address::new_unique()));
    expect_code(&env.init_config(), code::NOT_AUTHORITY);
}

#[test]
fn test_init_config_program_data_no_authority_rejected() {
    let mut env = Env::new();
    env.set_upgrade_authority(None);
    expect_code(&env.init_config(), code::NOT_AUTHORITY);
}

#[test]
fn test_init_config_program_data_wrong_owner_rejected() {
    let mut env = Env::new();
    // A copy of the real ProgramData (admin as authority) owned by another program.
    let real = env.account(&Env::program_data());
    let forged = Address::new_unique();
    env.set(forged, Address::new_unique(), real.data.clone());
    let mut ix = ix_init_config(&env.admin.pubkey(), &env.payer.pubkey());
    ix.accounts[3] = AccountMeta::new_readonly(forged, false);
    let (p, a) = (env.payer.insecure_clone(), env.admin.insecure_clone());
    expect_code(&env.send(&[ix], &[&p, &a]), code::INVALID_ACCOUNT_OWNER);
}

#[test]
fn test_init_config_program_data_wrong_address_rejected() {
    let mut env = Env::new();
    // Same bytes, owned by the loader, at an address that is not the program's ProgramData.
    let real = env.account(&Env::program_data());
    let forged = Address::new_unique();
    env.set(forged, LOADER, real.data.clone());
    let mut ix = ix_init_config(&env.admin.pubkey(), &env.payer.pubkey());
    ix.accounts[3] = AccountMeta::new_readonly(forged, false);
    let (p, a) = (env.payer.insecure_clone(), env.admin.insecure_clone());
    expect_code(&env.send(&[ix], &[&p, &a]), code::INVALID_PDA);
}

#[test]
fn test_init_config_program_data_malformed_rejected() {
    let mut env = Env::new();
    let pd = Env::program_data();
    let mut acc = env.account(&pd);
    acc.data[0] = 2; // UpgradeableLoaderState::Program tag instead of ProgramData
    env.svm.set_account(pd, acc).unwrap();
    expect_code(&env.init_config(), code::INVALID_ACCOUNT_DATA);
}

#[test]
fn test_init_config_admin_not_signer_rejected() {
    let mut env = Env::new();
    let mut ix = ix_init_config(&env.admin.pubkey(), &env.payer.pubkey());
    ix.accounts[0].is_signer = false;
    let p = env.payer.insecure_clone();
    expect_code(&env.send(&[ix], &[&p]), code::MISSING_SIGNATURE);
}

#[test]
fn test_init_config_wrong_config_pda_rejected() {
    let mut env = Env::new();
    let mut ix = ix_init_config(&env.admin.pubkey(), &env.payer.pubkey());
    ix.accounts[2] = AccountMeta::new(Address::new_unique(), false);
    let (p, a) = (env.payer.insecure_clone(), env.admin.insecure_clone());
    expect_code(&env.send(&[ix], &[&p, &a]), code::INVALID_PDA);
}

#[test]
fn test_init_config_wrong_system_program_rejected() {
    let mut env = Env::new();
    let mut ix = ix_init_config(&env.admin.pubkey(), &env.payer.pubkey());
    ix.accounts[4] = AccountMeta::new_readonly(Address::new_unique(), false);
    let (p, a) = (env.payer.insecure_clone(), env.admin.insecure_clone());
    expect_code(&env.send(&[ix], &[&p, &a]), code::INVALID_SYSTEM_PROGRAM);
}

#[test]
fn test_unknown_instruction_and_bad_payload_rejected() {
    let mut env = Env::new();
    let mut ix = ix_init_config(&env.admin.pubkey(), &env.payer.pubkey());
    ix.data = vec![9];
    let (p, a) = (env.payer.insecure_clone(), env.admin.insecure_clone());
    expect_code(
        &env.send(&[ix.clone()], &[&p, &a]),
        code::INVALID_INSTRUCTION,
    );
    ix.data = vec![];
    expect_code(
        &env.send(&[ix.clone()], &[&p, &a]),
        code::INVALID_INSTRUCTION,
    );
    ix.data = vec![0, 1];
    expect_code(
        &env.send(&[ix.clone()], &[&p, &a]),
        code::INVALID_INSTRUCTION_DATA,
    );
    ix.data = vec![0];
    ix.accounts.truncate(4);
    expect_code(&env.send(&[ix], &[&p, &a]), code::NOT_ENOUGH_ACCOUNTS);
}

// ---- set_feed_config -------------------------------------------------------------------

fn feed_config(env: &Env, feed: [u8; 32]) -> FeedConfigData {
    bytemuck::pod_read_unaligned(&env.account(&feed_pda(feed)).data)
}

#[test]
fn test_set_feed_config_create_and_update() {
    let mut env = Env::new();
    env.init_config().unwrap();
    env.set_feed(feed_args(SOL)).unwrap();
    let f = feed_config(&env, SOL);
    assert_eq!(f.feed, SOL);
    assert_eq!(f.tolerance, TOLERANCE);
    assert_eq!(f.hdr.tag, 3);
    let bump = f.hdr.bump;
    env.set_feed(SetFeedConfigArgsExt::with(feed_args(SOL), |a| {
        a.tolerance = H
    }))
    .unwrap();
    let g = feed_config(&env, SOL);
    assert_eq!(g.tolerance, H);
    assert_eq!(g.hdr.bump, bump);
}

trait SetFeedConfigArgsExt: Sized {
    fn with(self, f: impl FnOnce(&mut Self)) -> Self;
}

impl SetFeedConfigArgsExt for varswap_program::layout::SetFeedConfigArgs {
    fn with(mut self, f: impl FnOnce(&mut Self)) -> Self {
        f(&mut self);
        self
    }
}

type Case = (
    &'static str,
    u32,
    Box<dyn Fn(&mut varswap_program::layout::SetFeedConfigArgs)>,
);

#[test]
fn test_set_feed_config_parameter_rejections() {
    let mut env = Env::new();
    env.init_config().unwrap();
    let cases: Vec<Case> = vec![
        (
            "tolerance 0",
            code::ZERO_TOLERANCE,
            Box::new(|a| a.tolerance = 0),
        ),
        (
            "backfill 0",
            code::ZERO_BACKFILL,
            Box::new(|a| a.backfill_deadline = 0),
        ),
        (
            "max_conf 0",
            code::INVALID_MAX_CONF,
            Box::new(|a| a.max_conf = 0),
        ),
        (
            "max_conf > 100%",
            code::INVALID_MAX_CONF,
            Box::new(|a| a.max_conf = 10_001),
        ),
        (
            "min_interval 0",
            code::INVALID_FEED_CONFIG,
            Box::new(|a| a.min_interval = 0),
        ),
        (
            "max_buckets 1",
            code::INVALID_FEED_CONFIG,
            Box::new(|a| a.max_buckets = 1),
        ),
        (
            "max_buckets > MAX_BUCKETS",
            code::INVALID_FEED_CONFIG,
            Box::new(|a| a.max_buckets = varswap_core::MAX_BUCKETS + 1),
        ),
        (
            "receiver zero",
            code::INVALID_FEED_CONFIG,
            Box::new(|a| a.receiver_program = [0; 32]),
        ),
        (
            "padding",
            code::INVALID_INSTRUCTION_DATA,
            Box::new(|a| a._pad = [1, 0]),
        ),
    ];
    for (name, want, m) in cases {
        let mut a = feed_args(SOL);
        m(&mut a);
        let r = env.set_feed(a);
        assert_eq!(err_code(&r), want, "{name}");
    }
    let mut a = feed_args(SOL);
    a.max_buckets = varswap_core::MAX_BUCKETS;
    env.set_feed(a).unwrap();
}

#[test]
fn test_set_feed_config_other_admin_rejected() {
    let mut env = Env::new();
    env.init_config().unwrap();
    let other = Keypair::new();
    env.svm.airdrop(&other.pubkey(), 1_000_000_000).unwrap();
    let ix = ix_set_feed_config(&other.pubkey(), &other.pubkey(), &feed_args(SOL));
    expect_code(&env.send(&[ix], &[&other]), code::NOT_AUTHORITY);
}

#[test]
fn test_set_feed_config_admin_not_signer_rejected() {
    let mut env = Env::new();
    env.init_config().unwrap();
    let mut ix = ix_set_feed_config(&env.admin.pubkey(), &env.payer.pubkey(), &feed_args(SOL));
    ix.accounts[0].is_signer = false;
    let p = env.payer.insecure_clone();
    expect_code(&env.send(&[ix], &[&p]), code::MISSING_SIGNATURE);
}

#[test]
fn test_set_feed_config_forged_config_rejected() {
    let mut env = Env::new();
    env.init_config().unwrap();
    let real = env.account(&config_pda());
    // The real bytes under another owner, and under this program at another address.
    for (owner, want) in [
        (Address::new_unique(), code::INVALID_ACCOUNT_OWNER),
        (PROGRAM_ID, code::INVALID_PDA),
    ] {
        let forged = Address::new_unique();
        env.set(forged, owner, real.data.clone());
        let mut ix = ix_set_feed_config(&env.admin.pubkey(), &env.payer.pubkey(), &feed_args(SOL));
        ix.accounts[2] = AccountMeta::new_readonly(forged, false);
        let (p, a) = (env.payer.insecure_clone(), env.admin.insecure_clone());
        expect_code(&env.send(&[ix], &[&p, &a]), want);
    }
    // A config-sized account of the program with another tag.
    let mut data = real.data.clone();
    data[0] = 3;
    env.set(config_pda(), PROGRAM_ID, data);
    expect_code(&env.set_feed(feed_args(SOL)), code::INVALID_ACCOUNT_DATA);
}

#[test]
fn test_set_feed_config_wrong_feed_pda_rejected() {
    let mut env = Env::new();
    env.init_config().unwrap();
    let mut ix = ix_set_feed_config(&env.admin.pubkey(), &env.payer.pubkey(), &feed_args(SOL));
    ix.accounts[3] = AccountMeta::new(feed_pda(XAU), false);
    let (p, a) = (env.payer.insecure_clone(), env.admin.insecure_clone());
    expect_code(&env.send(&[ix], &[&p, &a]), code::INVALID_PDA);
}

#[test]
fn test_set_feed_config_feed_account_of_other_program_rejected() {
    let mut env = Env::new();
    env.init_config().unwrap();
    env.set(feed_pda(SOL), Address::new_unique(), vec![0; 96]);
    expect_code(&env.set_feed(feed_args(SOL)), code::ALREADY_INITIALIZED);
}

#[test]
fn test_set_feed_config_update_with_other_feed_bytes_rejected() {
    let mut env = Env::new();
    env.init_config().unwrap();
    env.set_feed(feed_args(XAU)).unwrap();
    // The XAU config's bytes planted at the SOL PDA: the stored feed does not match.
    let xau = env.account(&feed_pda(XAU));
    env.set(feed_pda(SOL), PROGRAM_ID, xau.data.clone());
    expect_code(&env.set_feed(feed_args(SOL)), code::INVALID_PDA);
}
