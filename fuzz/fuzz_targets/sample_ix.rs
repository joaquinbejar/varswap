//! The program's `sample` instruction in LiteSVM, on arbitrary price update accounts, against
//! the same path computed off chain with `pyth-lite` and `varswap-core`.
//!
//! One environment (config, the SOL feed with the v1 parameters, one hourly window of 24
//! intervals) is built once per process and cloned per input. Each input posts a few price
//! update accounts (structured fields or raw bytes, owned by the receiver or not) at chosen
//! times. Invariants: the transaction succeeds exactly when the off-chain path accepts the
//! print, and the window account afterwards equals the off-chain window byte for byte. On
//! failure the transaction error must be exactly `InstructionError(0, Custom(code))` with the
//! code the program maps the off-chain error to (`varswap_program::error::{pyth, core}`), and
//! the window is unchanged: an abort, a runtime or resource failure, or another code is a
//! crash, never an accepted rejection.

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
use pinocchio::error::ProgramError;
use solana_instruction_error::InstructionError;
use solana_transaction_error::TransactionError;
use varswap_core::{accept_print, LnKernel, RawPrint, UnixTimestamp, Window};
use varswap_program::error::{core, pyth};

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
        let off_chain = (|| -> Result<(), ProgramError> {
            let now = UnixTimestamp::new(now);
            let mut w = Window::load(&mut model).map_err(core)?;
            let spec = *w.spec();
            let u = pyth_lite::PriceUpdate::load(owner.as_array(), &data, &spec.receiver_program())
                .map_err(pyth)?;
            let req = pyth_lite::PriceRequirements::new(spec.feed(), None, spec.max_conf().get());
            let p = u.verify(&req, now).map_err(pyth)?;
            let accepted = accept_print(
                &spec,
                &RawPrint {
                    feed: p.feed_id(),
                    price: i64::try_from(p.price().get())
                        .map_err(|_| core(varswap_core::CoreError::PriceOutOfRange))?,
                    conf: p.conf(),
                    exponent: p.exponent(),
                    publish_time: p.publish_time(),
                    prev_publish_time: p.prev_publish_time(),
                },
            )
            .map_err(core)?;
            w.sample_accepted(&accepted, now, &LnKernel).map_err(core)?;
            Ok(())
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
        match (on_chain, off_chain) {
            (Ok(_), Ok(())) => assert_eq!(after, model, "window differs from the off-chain path"),
            (Err(f), Err(ProgramError::Custom(want))) => {
                match f.err {
                    TransactionError::InstructionError(0, InstructionError::Custom(got))
                        if got == want => {}
                    other => panic!(
                        "on chain {other:?}, off chain Custom({want})\n{}",
                        f.meta.pretty_logs()
                    ),
                }
                assert_eq!(after, before, "a failed sample wrote the window");
            }
            (Err(f), Err(e)) => panic!("off chain error is not a custom code: {e:?}; on chain {:?}", f.err),
            (Ok(_), Err(e)) => panic!("on chain accepted, off chain rejected with {e:?}"),
            (Err(f), Ok(())) => panic!(
                "on chain rejected with {:?}, off chain accepted\n{}",
                f.err,
                f.meta.pretty_logs()
            ),
        }
    }
});
