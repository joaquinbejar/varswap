//! Milestone 6's replay: 25 real canonical SOL/USD prints (hourly, 2026-10-29, recorded from
//! Hermes) go through `varswap-keeper` end to end, as on a cluster. `HermesSource` reads the
//! recorded responses, and `ReceiverChain` posts each print through the Pyth receiver and its
//! Wormhole program (both dumped from devnet, `pyth-post/tests/fixtures/README.md`) and samples
//! it in the posting transaction. The window finalizes, its `sum_sq` and realized variance
//! equal `varswap-core` fed the same prints, and both agree with an `f64` reference. Run
//! `make program` first.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::float_arithmetic,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::disallowed_macros,
    clippy::result_large_err,
    clippy::panic_in_result_fn,
    missing_docs
)]

mod common;

use std::num::NonZeroU64;

use common::*;
use pyth_post::ix::{self as pix, PYTH_RECEIVER, WORMHOLE_PROGRAM};
use pyth_post::{PostError, Transport};
use solana_account::Account;
use solana_address::Address;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_transaction::Transaction;
use varswap_core::{
    window_size, Bps, LnKernel, Print, RiskParams, Seconds, UnixTimestamp, Window, WindowSpec,
    WindowState, SUM_SQ_SCALE_BITS, YEAR_SECONDS,
};
use varswap_keeper::{
    HermesConfig, HermesSource, HttpGet, HttpResponse, Keeper, KeeperError, ReceiverChain,
    HERMES_API_KEY_VAR,
};
use varswap_sdk::decode;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../pyth-post/tests/fixtures");

/// `b_0` of the recorded day, 2026-10-29T00:00:00Z; 24 hourly intervals.
const START: i64 = 1_793_232_000;
const N: u32 = 24;
const END: i64 = START + 3_600 * N as i64;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("{FIXTURES}/{name}")).unwrap()
}

/// Hermes over the recorded responses: `GET /v2/updates/price/{t}` answers the file recorded
/// for `t`, 404 otherwise, and every request must carry the key.
struct Recorded;

impl HttpGet for Recorded {
    fn get(&self, url: &str, headers: &[(String, String)]) -> Result<HttpResponse, KeeperError> {
        assert!(headers.contains(&("Authorization".into(), "Bearer test-key".into())));
        let t = url
            .split("/v2/updates/price/")
            .nth(1)
            .and_then(|rest| rest.split('?').next())
            .unwrap();
        Ok(
            match std::fs::read(format!("{FIXTURES}/hermes/sol_usd_2026_10_29/{t}.json")) {
                Ok(body) => HttpResponse {
                    status: 200,
                    body: String::from_utf8(body).unwrap(),
                },
                Err(_) => HttpResponse {
                    status: 404,
                    body: String::new(),
                },
            },
        )
    }
}

fn hermes() -> HermesSource<Recorded> {
    let config =
        HermesConfig::from_lookup(|n| (n == HERMES_API_KEY_VAR).then(|| "test-key".to_string()))
            .unwrap();
    HermesSource::new(config, Recorded)
}

/// The varswap test environment plus the devnet receiver, Wormhole, the receiver's config,
/// guardian set 1 and treasury 0, as a `pyth-post` transport.
struct Svm<'a>(&'a mut Env);

impl Svm<'_> {
    fn install(env: &mut Env) {
        env.svm
            .add_program(PYTH_RECEIVER, &fixture("pyth_receiver.so"))
            .unwrap();
        env.svm
            .add_program(WORMHOLE_PROGRAM, &fixture("wormhole.so"))
            .unwrap();
        env.set(
            pix::receiver_config(),
            PYTH_RECEIVER,
            fixture("receiver_config.bin"),
        );
        env.set(
            pix::guardian_set(1),
            WORMHOLE_PROGRAM,
            fixture("guardian_set_1.bin"),
        );
        env.svm
            .set_account(
                pix::treasury(0),
                Account {
                    lamports: 2_002_014_461,
                    data: vec![],
                    owner: pix::SYSTEM_PROGRAM,
                    executable: false,
                    rent_epoch: 0,
                },
            )
            .unwrap();
    }
}

impl Transport for Svm<'_> {
    fn now(&self) -> Result<i64, PostError> {
        Ok(self
            .0
            .svm
            .get_sysvar::<solana_clock::Clock>()
            .unix_timestamp)
    }

    fn account(&self, a: &Address) -> Result<Option<(Address, Vec<u8>)>, PostError> {
        Ok(self
            .0
            .svm
            .get_account(a)
            .filter(|acc| acc.lamports > 0)
            .map(|acc| (acc.owner, acc.data)))
    }

    fn rent_exempt(&self, len: usize) -> Result<u64, PostError> {
        Ok(self.0.svm.minimum_balance_for_rent_exemption(len))
    }

    fn send(
        &mut self,
        ixs: &[Instruction],
        payer: &Keypair,
        extra: Option<&Keypair>,
    ) -> Result<String, PostError> {
        self.0.svm.expire_blockhash();
        let blockhash = self.0.svm.latest_blockhash();
        let signers: Vec<&Keypair> = std::iter::once(payer).chain(extra).collect();
        let tx =
            Transaction::new_signed_with_payer(ixs, Some(&payer.pubkey()), &signers, blockhash);
        let signature = tx.signatures[0].to_string();
        self.0
            .svm
            .send_transaction(tx)
            .map(|_| signature)
            .map_err(|e| PostError::Chain(format!("{:?}: {}", e.err, e.meta.logs.join(" | "))))
    }
}

/// The 25 recorded prints, through `varswap-core` directly: `sum_sq` and the realized variance.
fn expected(prints: &[(i64, i64, i64)]) -> (u128, u128) {
    let spec = WindowSpec::try_new(
        pyth_lite::FeedId::new(SOL),
        UnixTimestamp::new(START),
        UnixTimestamp::new(END),
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
    let mut data = vec![0u8; window_size(N + 1).unwrap()];
    let mut w = Window::init(&mut data, spec, 0).unwrap();
    let now = UnixTimestamp::new(END + 600);
    for &(price, publish, prev) in prints {
        let p = Print::try_new(
            pyth_lite::FeedId::new(SOL),
            NonZeroU64::new(price as u64).unwrap(),
            -8,
            UnixTimestamp::new(publish),
            UnixTimestamp::new(prev),
        )
        .unwrap();
        w.sample(&p, now, &LnKernel).unwrap();
    }
    w.finalize(now, &LnKernel).unwrap();
    (w.sum_sq().raw(), w.realized_var().unwrap().unwrap())
}

/// The recorded prints from the fixtures: `(price, publish_time, prev_publish_time)`.
fn recorded_prints() -> Vec<(i64, i64, i64)> {
    (0..=N)
        .map(|k| {
            let t = START + 3_600 * i64::from(k);
            let body = fixture(&format!("hermes/sol_usd_2026_10_29/{t}.json"));
            let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
            let p = &v["parsed"][0];
            (
                p["price"]["price"].as_str().unwrap().parse().unwrap(),
                p["price"]["publish_time"].as_i64().unwrap(),
                p["metadata"]["prev_publish_time"].as_i64().unwrap(),
            )
        })
        .collect()
}

#[test]
fn test_replay_of_24_real_returns_finalizes_to_the_off_chain_value() {
    let mut env = Env::new();
    Svm::install(&mut env);
    env.set_time(START - 3_600);
    env.init_config().unwrap();
    env.set_feed(feed_args(SOL)).unwrap();
    env.create_window(SOL, START, END, H).unwrap();
    let window = window_pda(SOL, START, END, H);
    env.set_time(END + 600);

    let payer = env.payer.insecure_clone();
    let before = env.svm.get_account(&payer.pubkey()).unwrap().lamports;
    let source = hermes();
    let keeper = Keeper::new(PROGRAM_ID);
    let mut chain = ReceiverChain::new(Svm(&mut env), hermes(), payer.insecure_clone(), PROGRAM_ID);
    let report = keeper.tick(&mut chain, &source, &window).unwrap();
    assert_eq!(report.posted, N + 1, "{report:?}");
    assert_eq!(report.skipped, 0);
    assert_eq!(report.finalized, Some(WindowState::Finalized));
    // Three transactions per print (create + write, write + verify, post + sample + closes),
    // then the finalize.
    assert_eq!(chain.signatures().len(), 3 * (N as usize + 1) + 1);
    // A second tick has nothing to do.
    let again = keeper.tick(&mut chain, &source, &window).unwrap();
    assert!(again.done);
    drop(chain);

    // No account is left behind: the payer spent fees only (5,000 lamports per signature:
    // two on the first and the last transaction of each post, one on the other and on the
    // finalize).
    let spent = before - env.svm.get_account(&payer.pubkey()).unwrap().lamports;
    assert_eq!(spent, 5_000 * (5 * u64::from(N + 1) + 1));

    let a = env.account(&window);
    let info = decode::window(&PROGRAM_ID, &a.owner, &a.data).unwrap();
    let mut d = a.data.clone();
    let on_chain_sum = Window::load(&mut d).unwrap().sum_sq().raw();
    let prints = recorded_prints();
    let (sum_sq, realized) = expected(&prints);
    assert_eq!(on_chain_sum, sum_sq);
    assert_eq!(info.realized_var, Some(realized));

    // And the f64 reference: sum of squared log returns, annualized over the window.
    let reference: f64 = prints
        .windows(2)
        .map(|w| (w[1].0 as f64 / w[0].0 as f64).ln().powi(2))
        .sum();
    let got = on_chain_sum as f64 / 2f64.powi(SUM_SQ_SCALE_BITS as i32);
    assert!(
        (got - reference).abs() <= reference * 1e-12,
        "{got} vs {reference}"
    );
    let annual = reference * YEAR_SECONDS as f64 / (END - START) as f64;
    let got_annual = realized as f64 / 2f64.powi(64);
    assert!(
        (got_annual - annual).abs() <= annual * 1e-9,
        "{got_annual} vs {annual}"
    );
    eprintln!(
        "SOL/USD 2026-10-29: sum of squared hourly log returns {reference:.12e}, annualized variance {annual:.9} (volatility {:.4}%)",
        annual.sqrt() * 100.0
    );
}
