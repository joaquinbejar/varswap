//! The program's `sample` instruction in LiteSVM, on arbitrary price update accounts, against
//! the same path computed off chain with `pyth-lite` and `varswap-core`.
//!
//! One environment (config, the SOL feed with the v1 parameters, one hourly window of 24
//! intervals) is built once per process and cloned per input. Each input posts a few price
//! update accounts (structured fields or raw bytes, owned by the receiver or not) at chosen
//! times. Invariants: the transaction succeeds exactly when the off-chain path accepts the
//! print, and the window account afterwards equals the off-chain window byte for byte (on
//! failure: unchanged).

#![no_main]
#![allow(dead_code, clippy::result_large_err)]

#[path = "../../program/tests/common/mod.rs"]
mod common;

use std::cell::RefCell;

use arbitrary::Arbitrary;
use common::{Env, PriceUpdate as Update, RECEIVER, SOL, T0, H};
use libfuzzer_sys::fuzz_target;
use litesvm::LiteSVM;
use solana_address::Address;
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_transaction::Transaction;
use varswap_core::{accept_print, LnKernel, RawPrint, UnixTimestamp, Window};

const N: u32 = 24;

#[derive(Debug, Arbitrary)]
enum Data<'a> {
    /// The receiver's layout with these fields.
    Fields {
        partial: Option<u8>,
        other_feed: bool,
        price: i64,
        conf: u64,
        exponent_off: u8,
        publish_off: i32,
        gap: u32,
    },
    /// Arbitrary bytes.
    Raw(&'a [u8]),
}

#[derive(Debug, Arbitrary)]
struct Step<'a> {
    data: Data<'a>,
    /// Owned by the receiver (otherwise by the system program).
    receiver_owned: bool,
    /// Clock time, seconds after the window's start.
    now_off: u32,
}

struct Base {
    svm: LiteSVM,
    payer: Keypair,
    window: Address,
}

thread_local! {
    static BASE: RefCell<Option<Base>> = const { RefCell::new(None) };
}

fn base() -> (LiteSVM, Keypair, Address) {
    BASE.with(|b| {
        let mut b = b.borrow_mut();
        let base = b.get_or_insert_with(|| {
            let (env, window) = Env::with_window(N);
            Base {
                svm: env.svm,
                payer: env.payer,
                window,
            }
        });
        (base.svm.clone(), base.payer.insecure_clone(), base.window)
    })
}

fn bytes(d: &Data<'_>) -> Vec<u8> {
    match d {
        Data::Raw(b) => b.to_vec(),
        Data::Fields {
            partial,
            other_feed,
            price,
            conf,
            exponent_off,
            publish_off,
            gap,
        } => {
            let start = T0 + i64::from(H);
            let publish = start + i64::from(*publish_off);
            let mut feed = SOL;
            if *other_feed {
                feed[0] ^= 1;
            }
            Update {
                partial: *partial,
                feed,
                price: *price,
                conf: *conf,
                exponent: -8 + i32::from(*exponent_off == 1),
                publish_time: publish,
                prev_publish_time: publish - i64::from(*gap),
            }
            .bytes()
        }
    }
}

fuzz_target!(|steps: Vec<Step<'_>>| {
    let (mut svm, payer, window) = base();
    let start = T0 + i64::from(H);
    for s in steps.iter().take(6) {
        let data = bytes(&s.data);
        let owner = if s.receiver_owned {
            RECEIVER
        } else {
            Address::new_from_array([0; 32])
        };
        let at = Address::new_unique();
        let lamports = svm.minimum_balance_for_rent_exemption(data.len());
        svm.set_account(
            at,
            solana_account::Account {
                lamports,
                data: data.clone(),
                owner,
                executable: false,
                rent_epoch: 0,
            },
        )
        .unwrap();
        let now = start + i64::from(s.now_off);
        let mut clock: solana_clock::Clock = svm.get_sysvar();
        clock.unix_timestamp = now;
        clock.slot += 1;
        svm.set_sysvar(&clock);

        // Off chain: the program's path on a copy of the window account.
        let mut model = svm.get_account(&window).unwrap().data;
        let off_chain = (|| {
            let mut w = Window::load(&mut model).ok()?;
            let spec = *w.spec();
            let u = pyth_lite::PriceUpdate::load(owner.as_array(), &data, &spec.receiver_program())
                .ok()?;
            let req = pyth_lite::PriceRequirements::new(spec.feed(), None, spec.max_conf().get());
            let p = u.verify(&req, UnixTimestamp::new(now)).ok()?;
            let accepted = accept_print(
                &spec,
                &RawPrint {
                    feed: p.feed_id(),
                    price: i64::try_from(p.price().get()).ok()?,
                    conf: p.conf(),
                    exponent: p.exponent(),
                    publish_time: p.publish_time(),
                    prev_publish_time: p.prev_publish_time(),
                },
            )
            .ok()?;
            w.sample_accepted(&accepted, UnixTimestamp::new(now), &LnKernel)
                .ok()
        })();

        let before = svm.get_account(&window).unwrap().data;
        svm.expire_blockhash();
        let tx = Transaction::new_signed_with_payer(
            &[common::ix_sample(&window, &at)],
            Some(&payer.pubkey()),
            &[&payer],
            svm.latest_blockhash(),
        );
        let on_chain = svm.send_transaction(tx);
        let after = svm.get_account(&window).unwrap().data;
        match (on_chain.is_ok(), off_chain.is_some()) {
            (true, true) => assert_eq!(after, model, "window differs from the off-chain path"),
            (false, false) => assert_eq!(after, before, "a failed sample wrote the window"),
            (c, o) => panic!("on chain ok = {c}, off chain ok = {o}: {on_chain:?}"),
        }
    }
});
