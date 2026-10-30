//! [`ReceiverChain`] over a recording transport: what it sends for one print, that it refuses
//! an update whose message is not the print before sending anything, and that a failed post
//! closes the encoded VAA.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::panic_in_result_fn
)]

use std::cell::RefCell;
use std::rc::Rc;

use pyth_post::PostError;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_transaction::Transaction;
use varswap_keeper::{Chain, KeeperError, PrintMsg, ReceiverChain, Transport, UpdateSource};
use varswap_sdk::Address;

const SOL: [u8; 32] = [
    0xef, 0x0d, 0x8b, 0x6f, 0xda, 0x2c, 0xeb, 0xa4, 0x1d, 0xa1, 0x5d, 0x40, 0x95, 0xd1, 0xda, 0x39,
    0x2a, 0x0d, 0x2f, 0x8e, 0xd0, 0xc6, 0xc7, 0xbc, 0x0f, 0x4c, 0xfa, 0xc8, 0xc2, 0x80, 0xb5, 0x6d,
];

fn print(publish_time: i64) -> PrintMsg {
    PrintMsg {
        price: 15_012_345_678,
        conf: 7_654_321,
        expo: -8,
        publish_time,
        prev_publish_time: publish_time - 1,
    }
}

/// A `PriceFeedMessage` written field by field from the pythnet-sdk definition.
fn message(feed: &[u8; 32], p: &PrintMsg) -> Vec<u8> {
    let mut m = vec![0];
    m.extend_from_slice(feed);
    m.extend_from_slice(&p.price.to_be_bytes());
    m.extend_from_slice(&p.conf.to_be_bytes());
    m.extend_from_slice(&p.expo.to_be_bytes());
    m.extend_from_slice(&p.publish_time.to_be_bytes());
    m.extend_from_slice(&p.prev_publish_time.to_be_bytes());
    m.extend_from_slice(&p.price.to_be_bytes()); // ema_price
    m.extend_from_slice(&p.conf.to_be_bytes()); // ema_conf
    m
}

/// A VAA v1 shape: version, guardian set index, `sigs` signatures of 66 bytes, a body.
fn vaa(guardian_set: u32, sigs: u8) -> Vec<u8> {
    let mut v = vec![1];
    v.extend_from_slice(&guardian_set.to_be_bytes());
    v.push(sigs);
    for i in 0..sigs {
        v.push(i);
        v.extend_from_slice(&[0xab; 65]);
    }
    v.extend_from_slice(&[0xcd; 88]); // header 51 + AUWV payload 37
    v
}

/// An accumulator update encoded independently of the parser, from the pythnet-sdk wire format.
fn pnau(vaa: &[u8], updates: &[(Vec<u8>, Vec<[u8; 20]>)]) -> Vec<u8> {
    let mut b = b"PNAU".to_vec();
    b.extend_from_slice(&[1, 0, 0, 0]); // major, minor, no trailing header, WormholeMerkle
    b.extend_from_slice(&u16::try_from(vaa.len()).unwrap().to_be_bytes());
    b.extend_from_slice(vaa);
    b.push(u8::try_from(updates.len()).unwrap());
    for (m, proof) in updates {
        b.extend_from_slice(&u16::try_from(m.len()).unwrap().to_be_bytes());
        b.extend_from_slice(m);
        b.push(u8::try_from(proof.len()).unwrap());
        for node in proof {
            b.extend_from_slice(node);
        }
    }
    b
}

fn proof(n: u8) -> Vec<[u8; 20]> {
    (0..n).map(|i| [i; 20]).collect()
}

/// The instructions of every transaction sent, with the number of extra signers.
type Sent = Rc<RefCell<Vec<(Vec<Instruction>, usize)>>>;

/// A transport that records every transaction and fails the `fail_at`-th send.
#[derive(Default)]
struct Recorder {
    sent: Sent,
    fail_at: Option<usize>,
    /// Accounts a landed transaction created and none closed (the encoded VAA).
    open: Vec<Address>,
}

impl Transport for Recorder {
    fn now(&self) -> Result<i64, PostError> {
        Ok(0)
    }
    fn account(&self, a: &Address) -> Result<Option<(Address, Vec<u8>)>, PostError> {
        Ok(self
            .open
            .contains(a)
            .then(|| (pyth_post::ix::WORMHOLE_PROGRAM, vec![])))
    }
    fn rent_exempt(&self, len: usize) -> Result<u64, PostError> {
        Ok(u64::try_from(len).unwrap() * 10)
    }
    fn send(
        &mut self,
        ixs: &[Instruction],
        payer: &Keypair,
        extra: Option<&Keypair>,
    ) -> Result<String, PostError> {
        let mut sent = self.sent.borrow_mut();
        let n = sent.len();
        sent.push((ixs.to_vec(), usize::from(extra.is_some())));
        let tx = Transaction::new_with_payer(ixs, Some(&payer.pubkey()));
        assert_eq!(
            usize::from(tx.message.header.num_required_signatures),
            1 + usize::from(extra.is_some()),
            "signers of send {n}"
        );
        if Some(n) == self.fail_at {
            return Err(PostError::Chain("rejected".into()));
        }
        for ix in ixs {
            if ix.program_id == pyth_post::ix::SYSTEM_PROGRAM {
                self.open.push(ix.accounts[1].pubkey);
            }
            if ix.data == [48, 221, 174, 198, 231, 7, 152, 38] {
                self.open.retain(|a| *a != ix.accounts[1].pubkey);
            }
        }
        Ok(format!("sig{n}"))
    }
}

struct Updates(Vec<u8>);

impl UpdateSource for Updates {
    fn update(&self, _: &[u8; 32], _: i64) -> Result<Vec<u8>, PostError> {
        Ok(self.0.clone())
    }
}

#[test]
fn test_receiver_chain_posts_samples_and_closes() {
    let p = print(1_793_404_800);
    let bytes = pnau(&vaa(4, 13), &[(message(&SOL, &p), proof(12))]);
    let rec = Recorder::default();
    let sent = Rc::clone(&rec.sent);
    let program = Address::new_unique();
    let mut chain = ReceiverChain::new(rec, Updates(bytes), Keypair::new(), program);
    let window = Address::new_unique();
    chain.post_sample(&window, &SOL, &p).unwrap();
    let sent = sent.borrow();
    assert_eq!(
        sent.len(),
        3,
        "create+init+write, write+verify, post+sample+closes"
    );
    assert_eq!(chain.signatures(), ["sig0", "sig1", "sig2"]);
    // The encoded VAA's rent is the transport's for 46 + 952 bytes.
    assert_eq!(&sent[0].0[0].data[4..12], &9_980_u64.to_le_bytes());
    let sample = &sent[2].0[1];
    assert_eq!(sample.program_id, program);
    assert_eq!(sample.accounts[0].pubkey, window);
    assert_eq!(sample.accounts[1].pubkey, sent[2].0[0].accounts[4].pubkey);
}

#[test]
fn test_receiver_chain_refuses_a_mismatched_update_before_sending() {
    let p = print(1_793_404_800);
    let mut other = p;
    other.price += 1;
    for bytes in [
        pnau(&vaa(4, 13), &[(message(&SOL, &other), proof(1))]),
        pnau(&vaa(4, 13), &[(message(&[3; 32], &p), proof(1))]),
        b"PNAU".to_vec(),
    ] {
        let rec = Recorder::default();
        let sent = Rc::clone(&rec.sent);
        let mut chain =
            ReceiverChain::new(rec, Updates(bytes), Keypair::new(), Address::new_unique());
        let err = chain
            .post_sample(&Address::new_unique(), &SOL, &p)
            .unwrap_err();
        assert!(matches!(err, KeeperError::Update(_)), "{err:?}");
        assert!(sent.borrow().is_empty());
    }
}

#[test]
fn test_failed_post_closes_the_encoded_vaa() {
    let p = print(1_793_404_800);
    let bytes = pnau(&vaa(4, 13), &[(message(&SOL, &p), proof(12))]);
    let rec = Recorder {
        fail_at: Some(1),
        ..Recorder::default()
    };
    let sent = Rc::clone(&rec.sent);
    let mut chain = ReceiverChain::new(rec, Updates(bytes), Keypair::new(), Address::new_unique());
    let err = chain
        .post_sample(&Address::new_unique(), &SOL, &p)
        .unwrap_err();
    assert_eq!(err, KeeperError::Chain("rejected".into()));
    let sent = sent.borrow();
    assert_eq!(sent.len(), 3, "create, the failed verify, then the close");
    let close = &sent[2].0;
    assert_eq!(close.len(), 1);
    assert_eq!(close[0].data, [48, 221, 174, 198, 231, 7, 152, 38]);
    assert_eq!(close[0].accounts[1].pubkey, sent[0].0[0].accounts[1].pubkey);
}
