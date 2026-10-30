//! The posting path without a cluster: the accumulator update parser against an independent
//! encoder, the instruction layouts against pyth-solana-receiver-sdk's Borsh types, and the post
//! plan (every transaction under the packet limit, the VAA written exactly once, in order).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros
)]

use anchor_lang::prelude::{AnchorDeserialize, AnchorSerialize};
use pyth_post::accumulator::{price_feed_message, AccumulatorUpdate, MerkleUpdate};
use pyth_post::ix::{self as receiver, PYTH_RECEIVER, SYSTEM_PROGRAM, WORMHOLE_PROGRAM};
use pyth_post::{post_plan, Extra, PostAccounts, PostError, PriceFeedMessage, FIRST_CHUNK};
use solana_address::Address;
use solana_instruction::{AccountMeta, Instruction};
use solana_transaction::Transaction;

const PACKET_DATA_SIZE: usize = 1232;

const SOL: [u8; 32] = [
    0xef, 0x0d, 0x8b, 0x6f, 0xda, 0x2c, 0xeb, 0xa4, 0x1d, 0xa1, 0x5d, 0x40, 0x95, 0xd1, 0xda, 0x39,
    0x2a, 0x0d, 0x2f, 0x8e, 0xd0, 0xc6, 0xc7, 0xbc, 0x0f, 0x4c, 0xfa, 0xc8, 0xc2, 0x80, 0xb5, 0x6d,
];

fn print(publish_time: i64) -> PriceFeedMessage {
    PriceFeedMessage {
        feed_id: SOL,
        price: 15_012_345_678,
        conf: 7_654_321,
        exponent: -8,
        publish_time,
        prev_publish_time: publish_time - 1,
        ema_price: 15_000_000_000,
        ema_conf: 7_000_000,
    }
}

/// A `PriceFeedMessage` written field by field from the pythnet-sdk definition.
fn message(feed: &[u8; 32], p: &PriceFeedMessage) -> Vec<u8> {
    let mut m = vec![0];
    m.extend_from_slice(feed);
    m.extend_from_slice(&p.price.to_be_bytes());
    m.extend_from_slice(&p.conf.to_be_bytes());
    m.extend_from_slice(&p.exponent.to_be_bytes());
    m.extend_from_slice(&p.publish_time.to_be_bytes());
    m.extend_from_slice(&p.prev_publish_time.to_be_bytes());
    m.extend_from_slice(&p.ema_price.to_be_bytes());
    m.extend_from_slice(&p.ema_conf.to_be_bytes());
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

fn to_vec<T: AnchorSerialize>(value: &T) -> Vec<u8> {
    let mut out = Vec::new();
    value.serialize(&mut out).unwrap();
    out
}

fn proof(n: u8) -> Vec<[u8; 20]> {
    (0..n).map(|i| [i; 20]).collect()
}

#[test]
fn test_parse_finds_the_feed_message_and_its_proof() {
    let other = [7; 32];
    let p = print(1_793_404_800);
    let v = vaa(4, 13);
    let bytes = pnau(
        &v,
        &[
            (
                message(
                    &other,
                    &PriceFeedMessage {
                        feed_id: other,
                        ..print(1)
                    },
                ),
                proof(3),
            ),
            (message(&SOL, &p), proof(11)),
        ],
    );
    let acc = AccumulatorUpdate::parse(&bytes).unwrap();
    assert_eq!(acc.vaa, v);
    assert_eq!(acc.updates.len(), 2);
    assert_eq!(acc.guardian_set_index().unwrap(), 4);
    let (u, decoded) = acc.find(&SOL).unwrap();
    assert_eq!(decoded, p);
    assert_eq!(u.proof, proof(11));
    assert_eq!(price_feed_message(&u.message).unwrap(), p);
    assert!(acc.find(&[9; 32]).is_none());
}

#[test]
fn test_parse_rejects_every_truncation_and_bad_framing() {
    let bytes = pnau(&vaa(4, 13), &[(message(&SOL, &print(10)), proof(4))]);
    for n in 0..bytes.len() {
        assert!(
            matches!(
                AccumulatorUpdate::parse(&bytes[..n]),
                Err(PostError::Update(_))
            ),
            "prefix {n}"
        );
    }
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(AccumulatorUpdate::parse(&trailing).is_err());
    let mut magic = bytes.clone();
    magic[0] = b'X';
    assert!(AccumulatorUpdate::parse(&magic).is_err());
    let mut major = bytes.clone();
    major[4] = 2;
    assert!(AccumulatorUpdate::parse(&major).is_err());
    let mut tag = bytes;
    tag[7] = 1;
    assert!(AccumulatorUpdate::parse(&tag).is_err());
}

/// The only real Hermes binary we hold is the start of an update (keeper/tests/fixtures): its
/// header matches the format (version 1.0, no trailing header, a 952-byte VAA under guardian
/// set 4 with 13 signatures) and the parser reports it as truncated.
#[test]
fn test_real_header_matches_the_format() {
    use base64::Engine;
    let head = base64::engine::general_purpose::STANDARD
        .decode("UE5BVQEAAAADuAEAAAAEDQ==")
        .unwrap();
    assert_eq!(&head[..4], b"PNAU");
    assert_eq!(&head[4..8], &[1, 0, 0, 0]);
    assert_eq!(u16::from_be_bytes([head[8], head[9]]), 952);
    assert_eq!(head[10], 1);
    assert_eq!(u32::from_be_bytes(head[11..15].try_into().unwrap()), 4);
    assert_eq!(head[15], 13);
    assert_eq!(952, 6 + 66 * 13 + 88);
    assert_eq!(
        AccumulatorUpdate::parse(&head),
        Err(PostError::Update("truncated accumulator update"))
    );
}

#[test]
fn test_price_message_rejects_other_shapes() {
    let m = message(&SOL, &print(5));
    assert!(price_feed_message(&m[..84]).is_err());
    let mut twap = m.clone();
    twap[0] = 1;
    assert!(price_feed_message(&twap).is_err());
    let mut long = m;
    long.push(0);
    assert!(price_feed_message(&long).is_err());
}

#[test]
fn test_post_update_data_is_the_receiver_sdk_borsh() {
    let u = MerkleUpdate {
        message: message(&SOL, &print(77)),
        proof: proof(9),
    };
    let (payer, vaa, pu) = (
        Address::new_unique(),
        Address::new_unique(),
        Address::new_unique(),
    );
    let ix = receiver::post_update(&payer, &vaa, &pu, &u, 0).unwrap();
    assert_eq!(ix.program_id, PYTH_RECEIVER);
    assert_eq!(&ix.data[..8], &[133, 95, 207, 175, 11, 79, 118, 44]);
    let params = pyth_solana_receiver_sdk::PostUpdateParams::try_from_slice(&ix.data[8..]).unwrap();
    assert_eq!(params.treasury_id, 0);
    assert_eq!(to_vec(&params), ix.data[8..]);
    // The proof and message round-trip through the SDK's own types.
    let mut expected = Vec::new();
    expected.extend_from_slice(&85_u32.to_le_bytes());
    expected.extend_from_slice(&u.message);
    expected.extend_from_slice(&9_u32.to_le_bytes());
    for n in &u.proof {
        expected.extend_from_slice(n);
    }
    assert_eq!(to_vec(&params.merkle_price_update), expected);
    let metas: Vec<(Address, bool, bool)> = ix
        .accounts
        .iter()
        .map(|m| (m.pubkey, m.is_signer, m.is_writable))
        .collect();
    assert_eq!(
        metas,
        vec![
            (payer, true, true),
            (vaa, false, false),
            (receiver::receiver_config(), false, false),
            (receiver::treasury(0), false, true),
            (pu, true, true),
            (SYSTEM_PROGRAM, false, false),
            (payer, true, false),
        ]
    );
    // The receiver's config PDA is the one read on devnet.
    assert_eq!(
        receiver::receiver_config().to_string(),
        "DaWUKXCyXsnzcvLUyeJRWou8KTn7XtadgTsdhJ6RHS7b"
    );
}

#[test]
fn test_wormhole_instruction_layouts() {
    let (a, v) = (Address::new_unique(), Address::new_unique());
    let w = receiver::write_encoded_vaa(&a, &v, 800, &[1, 2, 3]).unwrap();
    assert_eq!(w.program_id, WORMHOLE_PROGRAM);
    assert_eq!(
        w.data,
        [
            &[199, 208, 110, 177, 150, 76, 118, 42][..],
            &800_u32.to_le_bytes(),
            &3_u32.to_le_bytes(),
            &[1, 2, 3]
        ]
        .concat()
    );
    let verify = receiver::verify_encoded_vaa_v1(&a, &v, 4);
    assert_eq!(verify.accounts[2].pubkey, receiver::guardian_set(4));
    let c = receiver::create_account(&a, &v, 9, 998, &WORMHOLE_PROGRAM);
    assert_eq!(c.data.len(), 52);
    assert_eq!(&c.data[12..20], &998_u64.to_le_bytes());
    assert_eq!(&c.data[20..], WORMHOLE_PROGRAM.as_ref());
    assert_eq!(
        receiver::compute_unit_limit(1_000_000).data,
        [&[2][..], &1_000_000_u32.to_le_bytes()].concat()
    );
}

/// Bytes of a signed legacy transaction on the wire.
fn wire_size(ixs: &[Instruction], payer: &Address) -> usize {
    fn short_vec(n: usize) -> usize {
        match n {
            0..=0x7f => 1,
            0x80..=0x3fff => 2,
            _ => 3,
        }
    }
    let tx = Transaction::new_with_payer(ixs, Some(payer));
    let m = &tx.message;
    let sigs = usize::from(m.header.num_required_signatures);
    let mut n = short_vec(sigs) + 64 * sigs + 3;
    n += short_vec(m.account_keys.len()) + 32 * m.account_keys.len() + 32;
    n += short_vec(m.instructions.len());
    for ix in &m.instructions {
        n += 1 + short_vec(ix.accounts.len()) + ix.accounts.len();
        n += short_vec(ix.data.len()) + ix.data.len();
    }
    n
}

#[test]
fn test_post_plan_fits_the_packet_limit_and_writes_the_vaa_once() {
    for (sigs, proof_len) in [(13, 12), (19, 16), (1, 0)] {
        let v = vaa(4, sigs);
        let u = MerkleUpdate {
            message: message(&SOL, &print(1)),
            proof: proof(proof_len),
        };
        let acc = AccumulatorUpdate {
            vaa: v.clone(),
            updates: vec![u.clone()],
        };
        let (program, payer, ev, pu) = (
            Address::new_unique(),
            Address::new_unique(),
            Address::new_unique(),
            Address::new_unique(),
        );
        // The caller's instruction that reads the price update (varswap `sample`).
        let consume = Instruction {
            program_id: program,
            accounts: vec![
                AccountMeta::new(Address::new_unique(), false),
                AccountMeta::new_readonly(pu, false),
            ],
            data: vec![3],
        };
        let steps = post_plan(
            &PostAccounts {
                payer: &payer,
                encoded_vaa: &ev,
                price_update: &pu,
                encoded_vaa_rent: 1,
                treasury_id: 0,
            },
            &acc,
            &u,
            vec![consume],
        )
        .unwrap();
        // Create + init + first write, then writes, the verify, then post + sample + closes.
        assert_eq!(steps[0].extra, Extra::EncodedVaa);
        assert_eq!(steps.last().unwrap().extra, Extra::PriceUpdate);
        let mut written = Vec::new();
        for step in &steps {
            let size = wire_size(&step.instructions, &payer);
            assert!(size <= PACKET_DATA_SIZE, "{sigs} sigs: {size} bytes");
            for ix in &step.instructions {
                if ix.program_id == WORMHOLE_PROGRAM
                    && ix.data[..8] == [199, 208, 110, 177, 150, 76, 118, 42]
                {
                    let offset = u32::from_le_bytes(ix.data[8..12].try_into().unwrap());
                    assert_eq!(offset as usize, written.len());
                    written.extend_from_slice(&ix.data[16..]);
                }
            }
        }
        assert_eq!(written, v, "{sigs} sigs");
        assert!(steps[0].instructions[2].data.len() - 16 <= FIRST_CHUNK);
        let verify = &steps[steps.len() - 2].instructions;
        assert_eq!(verify[0].program_id, receiver::COMPUTE_BUDGET_PROGRAM);
        assert_eq!(
            verify.last().unwrap().data,
            [103, 56, 177, 229, 240, 103, 68, 73]
        );
        let last: Vec<Address> = steps
            .last()
            .unwrap()
            .instructions
            .iter()
            .map(|i| i.program_id)
            .collect();
        assert_eq!(
            last,
            vec![PYTH_RECEIVER, program, PYTH_RECEIVER, WORMHOLE_PROGRAM]
        );
    }
}

#[test]
fn test_rpc_transport_hides_the_url() {
    use solana_rpc_client::rpc_client::RpcClient;
    let url = "https://rpc.example/v1/TOKENPATH123/?api-key=SECRETKEY99";
    let t = pyth_post::RpcTransport::with_client(RpcClient::new_mock(url.to_string()));
    let shown = format!("{t:?}");
    assert!(!shown.contains("SECRETKEY99") && !shown.contains("rpc.example"));
}
