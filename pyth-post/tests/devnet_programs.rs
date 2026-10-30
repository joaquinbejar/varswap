//! The posting path against the deployed programs: the Pyth receiver and its Wormhole program,
//! dumped from devnet, with the receiver's config and guardian set 1 as they are on devnet,
//! fed real Hermes updates (`tests/fixtures/README.md`).

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
use pyth_post::{post, post_plan, AccumulatorUpdate, PostAccounts, PostError, Transport};
use solana_account::Account;
use solana_address::Address;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_transaction::Transaction;

const SOL: [u8; 32] = [
    0xef, 0x0d, 0x8b, 0x6f, 0xda, 0x2c, 0xeb, 0xa4, 0x1d, 0xa1, 0x5d, 0x40, 0x95, 0xd1, 0xda, 0x39,
    0x2a, 0x0d, 0x2f, 0x8e, 0xd0, 0xc6, 0xc7, 0xbc, 0x0f, 0x4c, 0xfa, 0xc8, 0xc2, 0x80, 0xb5, 0x6d,
];

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

/// The first boundary of the recorded day, 2026-10-29T00:00:00Z.
const T0: i64 = 1_793_232_000;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("{FIXTURES}/{name}")).unwrap()
}

/// Hermes' binary update for the hourly boundary `t` of the recorded day.
fn hermes_update(t: i64) -> Vec<u8> {
    let body = fixture(&format!("hermes/sol_usd_2026_10_29/{t}.json"));
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let data = json["binary"]["data"][0].as_str().unwrap();
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .unwrap()
}

/// LiteSVM with the devnet receiver, Wormhole, the receiver config and guardian set 1.
struct Svm(LiteSVM);

impl Svm {
    fn new() -> (Self, Keypair) {
        let mut svm = LiteSVM::new();
        svm.add_program(PYTH_RECEIVER, &fixture("pyth_receiver.so"))
            .unwrap();
        svm.add_program(WORMHOLE_PROGRAM, &fixture("wormhole.so"))
            .unwrap();
        for (address, owner, name) in [
            (ix::receiver_config(), PYTH_RECEIVER, "receiver_config.bin"),
            (ix::guardian_set(1), WORMHOLE_PROGRAM, "guardian_set_1.bin"),
        ] {
            let data = fixture(name);
            let lamports = svm.minimum_balance_for_rent_exemption(data.len());
            svm.set_account(
                address,
                Account {
                    lamports,
                    data,
                    owner,
                    executable: false,
                    rent_epoch: 0,
                },
            )
            .unwrap();
        }
        // Treasury 0 as on devnet: a system account holding 2.002014461 SOL at the dump slot.
        svm.set_account(
            ix::treasury(0),
            Account {
                lamports: 2_002_014_461,
                data: vec![],
                owner: ix::SYSTEM_PROGRAM,
                executable: false,
                rent_epoch: 0,
            },
        )
        .unwrap();
        let mut clock: solana_clock::Clock = svm.get_sysvar();
        clock.unix_timestamp = T0 + 600;
        svm.set_sysvar(&clock);
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();
        (Self(svm), payer)
    }

    fn lamports(&self, a: &Address) -> u64 {
        self.0.get_account(a).map_or(0, |a| a.lamports)
    }
}

impl Transport for Svm {
    fn now(&self) -> Result<i64, PostError> {
        Ok(self.0.get_sysvar::<solana_clock::Clock>().unix_timestamp)
    }

    fn account(&self, a: &Address) -> Result<Option<(Address, Vec<u8>)>, PostError> {
        Ok(self
            .0
            .get_account(a)
            .filter(|acc| acc.lamports > 0)
            .map(|acc| (acc.owner, acc.data)))
    }

    fn rent_exempt(&self, len: usize) -> Result<u64, PostError> {
        Ok(self.0.minimum_balance_for_rent_exemption(len))
    }

    fn send(
        &mut self,
        ixs: &[Instruction],
        payer: &Keypair,
        extra: Option<&Keypair>,
    ) -> Result<String, PostError> {
        self.0.expire_blockhash();
        let blockhash = self.0.latest_blockhash();
        let tx = match extra {
            Some(k) => Transaction::new_signed_with_payer(
                ixs,
                Some(&payer.pubkey()),
                &[payer, k],
                blockhash,
            ),
            None => {
                Transaction::new_signed_with_payer(ixs, Some(&payer.pubkey()), &[payer], blockhash)
            }
        };
        let signature = tx.signatures[0].to_string();
        self.0
            .send_transaction(tx)
            .map(|_| signature)
            .map_err(|e| PostError::Chain(format!("{:?}: {}", e.err, e.meta.logs.join(" | "))))
    }
}

/// A real update becomes a `Full` `PriceUpdateV2` with exactly the Hermes message: the
/// receiver accepts our VAA writes, verification and merkle proof. Its rent and the encoded
/// VAA's come back on close.
#[test]
fn test_real_update_posts_a_full_price_update() {
    let (mut svm, payer) = Svm::new();
    let acc = AccumulatorUpdate::parse(&hermes_update(T0)).unwrap();
    assert_eq!(acc.guardian_set_index().unwrap(), 1);
    let (update, message) = acc.find(&SOL).unwrap();
    assert_eq!(
        (message.price, message.exponent, message.publish_time),
        (10_232_283_367, -8, T0)
    );
    let (vaa, pu) = (Keypair::new(), Keypair::new());
    let before = svm.lamports(&payer.pubkey());
    let mut steps = post_plan(
        &PostAccounts {
            payer: &payer.pubkey(),
            encoded_vaa: &vaa.pubkey(),
            price_update: &pu.pubkey(),
            encoded_vaa_rent: svm.rent_exempt(46 + acc.vaa.len()).unwrap(),
            treasury_id: 0,
        },
        &acc,
        update,
        vec![],
    )
    .unwrap();
    // Everything but the closes, to read the account the receiver wrote.
    let last = steps.pop().unwrap();
    for step in &steps {
        let extra = (step.extra == pyth_post::Extra::EncodedVaa).then_some(&vaa);
        svm.send(&step.instructions, &payer, extra).unwrap();
    }
    svm.send(&last.instructions[..1], &payer, Some(&pu))
        .unwrap();
    let (owner, data) = svm.account(&pu.pubkey()).unwrap().unwrap();
    let posted = PriceUpdate::load(&owner.to_bytes(), &data, &RECEIVER_PROGRAM_ID).unwrap();
    assert_eq!(posted.verification_level(), VerificationLevel::Full);
    assert_eq!(posted.write_authority(), payer.pubkey().to_bytes());
    let m = posted.message();
    assert_eq!(
        (m.feed_id().to_bytes(), m.price(), m.conf(), m.exponent()),
        (SOL, message.price, message.conf, message.exponent)
    );
    assert_eq!(m.publish_time().get(), message.publish_time);
    assert_eq!(m.prev_publish_time().get(), message.prev_publish_time);
    svm.send(&last.instructions[1..], &payer, None).unwrap();
    assert!(svm.account(&pu.pubkey()).unwrap().is_none());
    assert!(svm.account(&vaa.pubkey()).unwrap().is_none());
    // Only fees were spent: 5,000 lamports per signature (the receiver's fee is 0).
    let spent = before - svm.lamports(&payer.pubkey());
    assert!(spent <= 5_000 * 8, "spent {spent}");
}

/// `post` over every recorded update: each one posts, runs the caller's instruction against
/// the fresh account in the same transaction, and closes both accounts.
#[test]
fn test_post_every_recorded_update() {
    let (mut svm, payer) = Svm::new();
    for k in 0..25 {
        let t = T0 + 3_600 * k;
        let acc = AccumulatorUpdate::parse(&hermes_update(t)).unwrap();
        let (update, message) = acc.find(&SOL).unwrap();
        assert_eq!(message.publish_time, t);
        assert!(message.prev_publish_time < t);
        let mut seen = None;
        let signatures = post(&mut svm, &payer, &acc, update, 0, |pu| {
            seen = Some(*pu);
            // Any instruction that reads the account: a zero transfer naming it read-only
            // would do; the receiver's own reclaim is the strictest check.
            vec![]
        })
        .unwrap();
        assert_eq!(signatures.len(), 3, "boundary {k}");
        assert!(svm.account(&seen.unwrap()).unwrap().is_none());
    }
}

/// A VAA with one changed byte fails Wormhole's signature check; the post errors and the
/// encoded VAA account is closed again.
#[test]
fn test_tampered_vaa_is_rejected_and_cleaned_up() {
    let (mut svm, payer) = Svm::new();
    let mut acc = AccumulatorUpdate::parse(&hermes_update(T0)).unwrap();
    let last = acc.vaa.len() - 1;
    acc.vaa[last] ^= 1; // the merkle root
    let update = acc.updates[0].clone();
    let before = svm.lamports(&payer.pubkey());
    let err = post(&mut svm, &payer, &acc, &update, 0, |_| vec![]).unwrap_err();
    assert!(matches!(err, PostError::Chain(_)), "{err:?}");
    assert!(before - svm.lamports(&payer.pubkey()) <= 5_000 * 4);
}

/// A message changed after the VAA was signed fails the receiver's merkle proof.
#[test]
fn test_tampered_message_fails_the_merkle_proof() {
    let (mut svm, payer) = Svm::new();
    let acc = AccumulatorUpdate::parse(&hermes_update(T0)).unwrap();
    let mut update = acc.updates[0].clone();
    update.message[40] ^= 1; // the price
    let err = post(&mut svm, &payer, &acc, &update, 0, |_| vec![]).unwrap_err();
    assert!(matches!(err, PostError::Chain(_)), "{err:?}");
}

/// A transport whose sends land on the real programs but whose confirmations fail at chosen
/// sends (`lie`: the transaction lands, the call returns `Err`) or that drops sends (`drop_from`:
/// the transaction never reaches the chain).
struct Faulty {
    inner: Svm,
    sends: usize,
    lie: Vec<usize>,
    drop_from: Option<usize>,
    closes: usize,
}

impl Faulty {
    fn new(inner: Svm, lie: &[usize], drop_from: Option<usize>) -> Self {
        Self {
            inner,
            sends: 0,
            lie: lie.to_vec(),
            drop_from,
            closes: 0,
        }
    }
}

impl Transport for Faulty {
    fn now(&self) -> Result<i64, PostError> {
        self.inner.now()
    }
    fn account(&self, a: &Address) -> Result<Option<(Address, Vec<u8>)>, PostError> {
        self.inner.account(a)
    }
    fn rent_exempt(&self, len: usize) -> Result<u64, PostError> {
        self.inner.rent_exempt(len)
    }
    fn send(
        &mut self,
        ixs: &[Instruction],
        payer: &Keypair,
        extra: Option<&Keypair>,
    ) -> Result<String, PostError> {
        let n = self.sends;
        self.sends += 1;
        if ixs.len() == 1 && ixs[0].data == [48, 221, 174, 198, 231, 7, 152, 38] {
            self.closes += 1;
        }
        if self.drop_from.is_some_and(|d| n >= d) {
            return Err(PostError::Chain("dropped".into()));
        }
        let landed = self.inner.send(ixs, payer, extra);
        if self.lie.contains(&n) {
            return Err(PostError::Chain("confirmation lost".into()));
        }
        landed
    }
}

/// The encoded VAA accounts a post opened, from the payer's create instructions: none may be
/// left once `post` returns.
fn open_vaas(svm: &Svm, payer: &Address) -> usize {
    // Every encoded VAA is owned by Wormhole and written by `payer` (bytes 9..41).
    svm.0
        .get_program_accounts(&WORMHOLE_PROGRAM)
        .iter()
        .filter(|(_, a)| a.lamports > 0 && a.data.get(9..41) == Some(payer.as_ref()))
        .count()
}

/// Lost confirmations at each step: the transaction landed, `post` errs, and the encoded VAA
/// is closed anyway (re-read, found, closed), for fees only. When the last transaction (which
/// closes everything itself) is the one that lied, no close is sent at all.
#[test]
fn test_lost_confirmations_never_leave_the_encoded_vaa_open() {
    for (lie_at, closes) in [(0, 1), (1, 1), (2, 0)] {
        let (svm, payer) = Svm::new();
        let before = svm.lamports(&payer.pubkey());
        let mut t = Faulty::new(svm, &[lie_at], None);
        let acc = AccumulatorUpdate::parse(&hermes_update(T0)).unwrap();
        let update = acc.updates[0].clone();
        let err = post(&mut t, &payer, &acc, &update, 0, |_| vec![]).unwrap_err();
        assert_eq!(
            err,
            PostError::Chain("confirmation lost".into()),
            "lie at {lie_at}"
        );
        assert_eq!(t.closes, closes, "lie at {lie_at}");
        assert_eq!(open_vaas(&t.inner, &payer.pubkey()), 0, "lie at {lie_at}");
        let spent = before - t.inner.lamports(&payer.pubkey());
        assert!(spent <= 5_000 * 6, "lie at {lie_at}: spent {spent}");
    }
}

/// The close itself lands but its confirmation is lost: the re-read sees the account gone, so
/// the close is not retried.
#[test]
fn test_a_landed_close_is_not_retried() {
    let (svm, payer) = Svm::new();
    // Send 1 (verify) is dropped, send 2 (the close) lands and lies.
    let t = Faulty::new(svm, &[2], None);
    let acc = AccumulatorUpdate::parse(&hermes_update(T0)).unwrap();
    let update = acc.updates[0].clone();
    struct DropOne(Faulty);
    impl Transport for DropOne {
        fn now(&self) -> Result<i64, PostError> {
            self.0.now()
        }
        fn account(&self, a: &Address) -> Result<Option<(Address, Vec<u8>)>, PostError> {
            self.0.account(a)
        }
        fn rent_exempt(&self, len: usize) -> Result<u64, PostError> {
            self.0.rent_exempt(len)
        }
        fn send(
            &mut self,
            ixs: &[Instruction],
            payer: &Keypair,
            extra: Option<&Keypair>,
        ) -> Result<String, PostError> {
            if self.0.sends == 1 {
                self.0.sends += 1;
                return Err(PostError::Chain("verify dropped".into()));
            }
            self.0.send(ixs, payer, extra)
        }
    }
    let mut t = DropOne(t);
    let err = post(&mut t, &payer, &acc, &update, 0, |_| vec![]).unwrap_err();
    assert_eq!(err, PostError::Chain("verify dropped".into()));
    assert_eq!(t.0.closes, 1, "one close, not retried");
    assert_eq!(open_vaas(&t.0.inner, &payer.pubkey()), 0);
}

/// Every send after the first is dropped: the encoded VAA cannot be closed, and the error is
/// `Orphaned` with its address after the bounded attempts. Once sends go through again,
/// `close_if_open` closes it.
#[test]
fn test_an_unclosable_encoded_vaa_is_reported_and_closed_later() {
    let (svm, payer) = Svm::new();
    let mut t = Faulty::new(svm, &[], Some(1));
    let acc = AccumulatorUpdate::parse(&hermes_update(T0)).unwrap();
    let update = acc.updates[0].clone();
    let err = post(&mut t, &payer, &acc, &update, 0, |_| vec![]).unwrap_err();
    let PostError::Orphaned { encoded_vaa, cause } = err else {
        panic!("{err:?}")
    };
    assert_eq!(cause, "chain operation failed: dropped");
    assert_eq!(t.closes, pyth_post::CLOSE_ATTEMPTS);
    assert_eq!(open_vaas(&t.inner, &payer.pubkey()), 1);
    assert!(t.inner.account(&encoded_vaa).unwrap().is_some());

    t.drop_from = None;
    assert_eq!(pyth_post::close_if_open(&mut t, &payer, &encoded_vaa), None);
    assert_eq!(open_vaas(&t.inner, &payer.pubkey()), 0);
    // Already closed: nothing more is sent.
    let closes = t.closes;
    assert_eq!(pyth_post::close_if_open(&mut t, &payer, &encoded_vaa), None);
    assert_eq!(t.closes, closes);
}
