//! metalbook #25's two receiver tests, ported onto `pyth-post`. A guardian-signed Hermes update
//! for XAU/USD (recorded by metalbook on 2026-10-30, `fixtures/metalbook/`) is written to an
//! encoded VAA, verified by Wormhole, posted as a `Full` `PriceUpdateV2` and closed, for fees
//! only. A changed guardian signature fails Wormhole's verification, and a changed price fails
//! the receiver's merkle proof (`InvalidPriceUpdate`, 6002). The receiver config and guardian
//! set fixtures are metalbook's JSON records of the same devnet accounts as
//! `fixtures/*.bin`, and the test checks that they match byte for byte.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros
)]

use base64::Engine;
use litesvm::LiteSVM;
use pyth_lite::{PriceUpdate, VerificationLevel, RECEIVER_PROGRAM_ID};
use pyth_post::ix::{self, PYTH_RECEIVER, WORMHOLE_PROGRAM};
use pyth_post::{post_plan, AccumulatorUpdate, Extra, PostAccounts};
use serde_json::Value;
use solana_account::Account;
use solana_address::Address;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_transaction::Transaction;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

const XAU: [u8; 32] = [
    0x76, 0x5d, 0x2b, 0xa9, 0x06, 0xdb, 0xc3, 0x2c, 0xa1, 0x7c, 0xc1, 0x1f, 0x53, 0x10, 0xa8, 0x9e,
    0x9e, 0xe1, 0xf6, 0x42, 0x05, 0x08, 0xc6, 0x38, 0x61, 0xf2, 0xf8, 0xba, 0x4e, 0xe3, 0x4b, 0xb2,
];
/// XAG/USD (`pyth-lite/fixtures/xag_usd.json`).
const XAG: [u8; 32] = [
    0xf2, 0xfb, 0x02, 0xc3, 0x2b, 0x05, 0x5c, 0x80, 0x5e, 0x72, 0x38, 0xd6, 0x28, 0xe5, 0xe9, 0xda,
    0xde, 0xf2, 0x74, 0x37, 0x61, 0x14, 0xeb, 0x1f, 0x01, 0x23, 0x37, 0xca, 0xbe, 0x93, 0x87, 0x1e,
];

fn json(name: &str) -> Value {
    serde_json::from_slice(&std::fs::read(format!("{FIXTURES}/metalbook/{name}")).unwrap()).unwrap()
}

fn b64(v: &Value) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(v.as_str().unwrap())
        .unwrap()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The recorded update and its parsed fields.
struct Recorded {
    update: Vec<u8>,
    price: i64,
    conf: u64,
    publish_time: i64,
    prev_publish_time: i64,
}

fn recorded() -> Recorded {
    let f = json("hermes_xau_update.json");
    assert_eq!(f["feed_id"].as_str().unwrap(), hex(&XAU));
    assert_eq!(f["expo"].as_i64().unwrap(), -3);
    Recorded {
        update: b64(&f["binary_base64"]),
        price: f["price"].as_i64().unwrap(),
        conf: f["conf"].as_u64().unwrap(),
        publish_time: f["publish_time"].as_i64().unwrap(),
        prev_publish_time: f["prev_publish_time"].as_i64().unwrap(),
    }
}

/// LiteSVM with the devnet receiver and Wormhole, the config and guardian set from metalbook's
/// records, and the treasury as a funded system account.
fn setup() -> (LiteSVM, Keypair) {
    let mut svm = LiteSVM::new();
    for (id, file) in [
        (PYTH_RECEIVER, "pyth_receiver.so"),
        (WORMHOLE_PROGRAM, "wormhole.so"),
    ] {
        svm.add_program(id, &std::fs::read(format!("{FIXTURES}/{file}")).unwrap())
            .unwrap();
    }
    for (name, bin, expected) in [
        (
            "receiver_config.json",
            "receiver_config.bin",
            ix::receiver_config(),
        ),
        (
            "guardian_set_1.json",
            "guardian_set_1.bin",
            ix::guardian_set(1),
        ),
    ] {
        let f = json(name);
        let address: Address = f["address"].as_str().unwrap().parse().unwrap();
        assert_eq!(address, expected, "{name}");
        let data = b64(&f["data_base64"]);
        // The same devnet account as the dump next to it.
        assert_eq!(
            data,
            std::fs::read(format!("{FIXTURES}/{bin}")).unwrap(),
            "{name}"
        );
        let account = Account {
            lamports: f["lamports"].as_u64().unwrap(),
            data,
            owner: f["owner"].as_str().unwrap().parse().unwrap(),
            executable: false,
            rent_epoch: 0,
        };
        svm.set_account(address, account).unwrap();
    }
    svm.airdrop(&ix::treasury(0), 1_000_000_000).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();
    (svm, payer)
}

type Sent = Result<u64, (String, Vec<String>)>;

fn send(svm: &mut LiteSVM, ixs: &[Instruction], signers: &[&Keypair]) -> Sent {
    svm.expire_blockhash();
    let tx = Transaction::new_signed_with_payer(
        ixs,
        Some(&signers[0].pubkey()),
        signers,
        svm.latest_blockhash(),
    );
    svm.send_transaction(tx)
        .map(|m| m.compute_units_consumed)
        .map_err(|e| (format!("{:?}", e.err), e.meta.logs))
}

/// The steps of one post with fresh temporary keypairs and no caller instruction; the last
/// step is split into `post_update` alone and the two closes, so the test can read the
/// account in between.
fn post(
    svm: &mut LiteSVM,
    payer: &Keypair,
    update: &AccumulatorUpdate,
) -> (Vec<Sent>, Sent, Address, Address) {
    let (vaa, pu) = (Keypair::new(), Keypair::new());
    let (message, _) = update.find(&XAU).unwrap();
    let mut steps = post_plan(
        &PostAccounts {
            payer: &payer.pubkey(),
            encoded_vaa: &vaa.pubkey(),
            price_update: &pu.pubkey(),
            encoded_vaa_rent: svm.minimum_balance_for_rent_exemption(46 + update.vaa.len()),
            treasury_id: 0,
        },
        update,
        message,
        vec![],
    )
    .unwrap();
    let last = steps.pop().unwrap();
    let mut verify = Vec::new();
    for step in &steps {
        let signers: Vec<&Keypair> = match step.extra {
            Extra::EncodedVaa => vec![payer, &vaa],
            _ => vec![payer],
        };
        verify.push(send(svm, &step.instructions, &signers));
    }
    let posted = send(svm, &last.instructions[..1], &[payer, &pu]);
    (verify, posted, vaa.pubkey(), pu.pubkey())
}

#[test]
fn test_a_recorded_hermes_update_is_verified_posted_and_closed() {
    let r = recorded();
    let update = AccumulatorUpdate::parse(&r.update).unwrap();
    assert_eq!(update.guardian_set_index().unwrap(), 1);
    assert!(update.find(&XAU).is_some());
    assert!(update.find(&XAG).is_none());

    let (mut svm, payer) = setup();
    let before = svm.get_balance(&payer.pubkey()).unwrap();
    let (verify, posted, vaa, pu) = post(&mut svm, &payer, &update);
    for (i, s) in verify.iter().enumerate() {
        let cu = s
            .as_ref()
            .unwrap_or_else(|(e, logs)| panic!("step {i}: {e}\n{}", logs.join("\n")));
        println!("CU step {i}: {cu}");
    }
    posted.unwrap_or_else(|(e, logs)| panic!("post_update: {e}\n{}", logs.join("\n")));

    let account = svm.get_account(&pu).unwrap();
    let p = PriceUpdate::load(
        &account.owner.to_bytes(),
        &account.data,
        &RECEIVER_PROGRAM_ID,
    )
    .unwrap();
    assert_eq!(p.verification_level(), VerificationLevel::Full);
    let m = p.message();
    assert_eq!(m.feed_id().to_bytes(), XAU);
    assert_eq!((m.price(), m.conf(), m.exponent()), (r.price, r.conf, -3));
    assert_eq!(m.publish_time().get(), r.publish_time);
    assert_eq!(m.prev_publish_time().get(), r.prev_publish_time);

    // Both temporary accounts closed, their rent back: only signature fees were paid.
    send(
        &mut svm,
        &[
            ix::reclaim_rent(&payer.pubkey(), &pu),
            ix::close_encoded_vaa(&payer.pubkey(), &vaa),
        ],
        &[&payer],
    )
    .unwrap();
    for a in [vaa, pu] {
        assert!(svm.get_account(&a).is_none_or(|a| a.lamports == 0));
    }
    let signatures = 2 + 1 + 2 + 1;
    assert_eq!(
        before - svm.get_balance(&payer.pubkey()).unwrap(),
        signatures * 5_000
    );
}

#[test]
fn test_tampered_updates_are_rejected_by_wormhole_or_the_receiver() {
    let r = recorded();
    let update = AccumulatorUpdate::parse(&r.update).unwrap();

    // A changed guardian signature: Wormhole's verification fails, nothing is posted. The VAA
    // fits the first write, so the verify step is [compute budget, verify]: instruction 1.
    let (mut svm, payer) = setup();
    let mut bad_sig = update.clone();
    bad_sig.vaa[10] ^= 1;
    let (verify, posted, _, pu) = post(&mut svm, &payer, &bad_sig);
    verify[0].as_ref().unwrap();
    let (e, logs) = verify[1].clone().unwrap_err();
    assert!(
        e.contains("InstructionError(1,"),
        "{e}: {}",
        logs.join("\n")
    );
    assert!(posted.is_err());
    assert!(svm.get_account(&pu).is_none_or(|a| a.lamports == 0));

    // A changed price in the message: the VAA verifies but the merkle proof does not.
    // `post_update` is instruction 0 of its transaction: the receiver's
    // `InvalidPriceUpdate` (6002).
    let (mut svm, payer) = setup();
    let mut bad_price = update;
    bad_price.updates[0].message[33 + 7] ^= 1; // low byte of the big-endian price
    let (verify, posted, _, pu) = post(&mut svm, &payer, &bad_price);
    for s in &verify {
        s.as_ref().unwrap();
    }
    let (e, logs) = posted.unwrap_err();
    assert!(e.contains("InstructionError(0, Custom(6002))"), "{e}");
    assert!(logs.iter().any(|l| l.contains("InvalidPriceUpdate")));
    assert!(svm.get_account(&pu).is_none_or(|a| a.lamports == 0));
}
