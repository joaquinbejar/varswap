//! Decoding and checks against real mainnet `PriceUpdateV2` accounts (`fixtures/`, fetched by
//! `fixtures/fetch.py`), with a negative test per check.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    missing_docs
)]

use pyth_lite::{
    FeedId, PriceRequirements, PriceUpdate, PythError, UnixTimestamp, VerificationLevel,
    PRICE_UPDATE_V2_DISCRIMINATOR, PRICE_UPDATE_V2_LEN, PUSH_ORACLE_PROGRAM_ID,
    RECEIVER_PROGRAM_ID,
};

struct Fixture {
    name: &'static str,
    data: &'static [u8],
    owner: &'static [u8; 32],
    feed_hex: &'static str,
}

const XAU: Fixture = Fixture {
    name: "xau_usd",
    data: include_bytes!("../fixtures/xau_usd.data.bin"),
    owner: include_bytes!("../fixtures/xau_usd.owner.bin"),
    feed_hex: "765d2ba906dbc32ca17cc11f5310a89e9ee1f6420508c63861f2f8ba4ee34bb2",
};
const XAG: Fixture = Fixture {
    name: "xag_usd",
    data: include_bytes!("../fixtures/xag_usd.data.bin"),
    owner: include_bytes!("../fixtures/xag_usd.owner.bin"),
    feed_hex: "f2fb02c32b055c805e7238d628e5e9dadef274376114eb1f012337cabe93871e",
};
const SOL: Fixture = Fixture {
    name: "sol_usd",
    data: include_bytes!("../fixtures/sol_usd.data.bin"),
    owner: include_bytes!("../fixtures/sol_usd.owner.bin"),
    feed_hex: "ef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d",
};
const PARTIAL: Fixture = Fixture {
    name: "partial",
    data: include_bytes!("../fixtures/partial.data.bin"),
    owner: include_bytes!("../fixtures/partial.owner.bin"),
    feed_hex: "e62df6c8b4a85fe1a67db44dc12de5db330f7ac66b72dc658afedf0f4a415b43",
};
const FULL: [Fixture; 3] = [XAU, XAG, SOL];

/// Offset of `price_message` in a `Full` update: discriminator, write authority, 1-byte tag.
const FULL_MSG: usize = 8 + 32 + 1;
const PRICE_AT: usize = FULL_MSG + 32;
const CONF_AT: usize = PRICE_AT + 8;
const PUBLISH_AT: usize = CONF_AT + 8 + 4;

fn hex32(s: &str) -> [u8; 32] {
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap();
    }
    out
}

fn base58_32(s: &str) -> [u8; 32] {
    const ALPHABET: &str = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    let mut out = [0u8; 32];
    for c in s.chars() {
        let mut carry = ALPHABET.find(c).unwrap() as u32;
        for b in out.iter_mut().rev() {
            carry += u32::from(*b) * 58;
            *b = (carry & 0xff) as u8;
            carry >>= 8;
        }
        assert_eq!(carry, 0);
    }
    out
}

fn feed(f: &Fixture) -> FeedId {
    FeedId::new(hex32(f.feed_hex))
}

fn load(f: &Fixture) -> PriceUpdate {
    PriceUpdate::load(f.owner, f.data, &RECEIVER_PROGRAM_ID).unwrap()
}

/// Requirements every Full fixture meets at its own publish time: 60 s, 10% confidence.
fn loose(f: &Fixture) -> PriceRequirements {
    PriceRequirements::new(feed(f), Some(60), 1_000)
}

fn publish(u: &PriceUpdate) -> UnixTimestamp {
    u.message().publish_time()
}

fn patched(data: &[u8], at: usize, bytes: &[u8]) -> Vec<u8> {
    let mut v = data.to_vec();
    v[at..at + bytes.len()].copy_from_slice(bytes);
    v
}

#[test]
fn test_ids_match_base58_and_receiver_sdk() {
    assert_eq!(
        RECEIVER_PROGRAM_ID,
        base58_32("rec5EKMGg6MxZYaMdyBfgwp4d5rB9T1VQH5pJv5LtFJ")
    );
    assert_eq!(
        PUSH_ORACLE_PROGRAM_ID,
        base58_32("pythWSnswVUd12oZpeFP8e9CVaEqJg25g1Vtc2biRsT")
    );
    assert_eq!(RECEIVER_PROGRAM_ID, pyth_solana_receiver_sdk::ID.to_bytes());
    assert_eq!(
        PUSH_ORACLE_PROGRAM_ID,
        pyth_solana_receiver_sdk::PYTH_PUSH_ORACLE_ID.to_bytes()
    );
    assert_eq!(
        PRICE_UPDATE_V2_LEN,
        pyth_solana_receiver_sdk::price_update::PriceUpdateV2::LEN
    );
}

#[test]
fn test_fixtures_owned_by_receiver_and_full_length() {
    for f in FULL.iter().chain([&PARTIAL]) {
        assert_eq!(*f.owner, RECEIVER_PROGRAM_ID, "{}", f.name);
        assert_eq!(f.data.len(), PRICE_UPDATE_V2_LEN, "{}", f.name);
        assert_eq!(f.data[..8], PRICE_UPDATE_V2_DISCRIMINATOR, "{}", f.name);
    }
}

#[test]
fn test_decode_full_fixtures_verify_ok() {
    for f in &FULL {
        let u = load(f);
        assert_eq!(
            u.verification_level(),
            VerificationLevel::Full,
            "{}",
            f.name
        );
        let m = u.message();
        assert_eq!(m.feed_id(), feed(f), "{}", f.name);
        assert!(m.price() > 0, "{}", f.name);
        assert!(m.prev_publish_time() <= m.publish_time(), "{}", f.name);
        let p = u.verify(&loose(f), publish(&u)).unwrap();
        assert_eq!(i64::try_from(p.price().get()).unwrap(), m.price());
        assert_eq!(p.conf(), m.conf());
        assert_eq!(p.exponent(), m.exponent());
        assert_eq!(p.publish_time(), m.publish_time());
        assert_eq!(p.prev_publish_time(), m.prev_publish_time());
        assert_eq!(p.feed_id(), feed(f));
    }
}

#[test]
fn test_decode_partial_fixture_verify_not_fully_verified() {
    let u = load(&PARTIAL);
    assert_eq!(
        u.verification_level(),
        VerificationLevel::Partial { num_signatures: 5 }
    );
    assert_eq!(u.message().feed_id(), feed(&PARTIAL));
    let req = PriceRequirements::new(feed(&PARTIAL), None, u16::MAX);
    assert_eq!(
        u.verify(&req, publish(&u)),
        Err(PythError::NotFullyVerified)
    );
}

#[test]
fn test_load_wrong_owner_rejected() {
    for f in FULL.iter().chain([&PARTIAL]) {
        for owner in [PUSH_ORACLE_PROGRAM_ID, [0u8; 32], {
            let mut o = RECEIVER_PROGRAM_ID;
            o[31] ^= 1;
            o
        }] {
            assert_eq!(
                PriceUpdate::load(&owner, f.data, &RECEIVER_PROGRAM_ID),
                Err(PythError::WrongOwner),
                "{}",
                f.name
            );
        }
    }
}

#[test]
fn test_decode_wrong_discriminator_rejected() {
    for f in FULL.iter().chain([&PARTIAL]) {
        for i in 0..8 {
            let mut d = f.data.to_vec();
            d[i] ^= 0x01;
            assert_eq!(
                PriceUpdate::decode(&d),
                Err(PythError::WrongDiscriminator),
                "{} byte {i}",
                f.name
            );
        }
    }
}

#[test]
fn test_decode_truncated_rejected_at_every_length() {
    // A Full update needs 133 bytes, a Partial one 134.
    for (f, needed) in [(&SOL, 133), (&XAU, 133), (&XAG, 133), (&PARTIAL, 134)] {
        for len in 0..needed {
            assert_eq!(
                PriceUpdate::decode(&f.data[..len]),
                Err(PythError::Truncated),
                "{} len {len}",
                f.name
            );
        }
        assert!(PriceUpdate::decode(&f.data[..needed]).is_ok(), "{}", f.name);
    }
}

#[test]
fn test_decode_unknown_verification_tag_rejected() {
    for tag in [2u8, 3, 0x7f, 0xff] {
        for f in FULL.iter().chain([&PARTIAL]) {
            let d = patched(f.data, 40, &[tag]);
            assert_eq!(
                PriceUpdate::decode(&d),
                Err(PythError::UnknownVerificationLevel),
                "{} tag {tag}",
                f.name
            );
        }
    }
}

#[test]
fn test_decode_trailing_bytes_ignored() {
    let mut d = SOL.data.to_vec();
    d.extend_from_slice(&[0xaa; 16]);
    assert_eq!(PriceUpdate::decode(&d), Ok(load(&SOL)));
}

#[test]
fn test_verify_full_downgraded_to_partial_rejected() {
    // The same SOL message re-encoded as Partial { 13 }: the message shifts by one byte.
    let mut d = SOL.data[..FULL_MSG].to_vec();
    d[40] = 0;
    d.push(13);
    d.extend_from_slice(&SOL.data[FULL_MSG..133]);
    let u = PriceUpdate::decode(&d).unwrap();
    assert_eq!(
        u.verification_level(),
        VerificationLevel::Partial { num_signatures: 13 }
    );
    assert_eq!(*u.message(), *load(&SOL).message());
    assert_eq!(
        u.verify(&loose(&SOL), publish(&u)),
        Err(PythError::NotFullyVerified)
    );
}

#[test]
fn test_verify_wrong_feed_rejected() {
    for f in &FULL {
        let u = load(f);
        for other in FULL.iter().filter(|o| o.name != f.name) {
            assert_eq!(
                u.verify(&loose(other), publish(&u)),
                Err(PythError::WrongFeed),
                "{} as {}",
                f.name,
                other.name
            );
        }
    }
}

#[test]
fn test_verify_non_positive_price_rejected() {
    for price in [0i64, -1, i64::MIN] {
        let d = patched(SOL.data, PRICE_AT, &price.to_le_bytes());
        let u = PriceUpdate::decode(&d).unwrap();
        assert_eq!(u.message().price(), price);
        assert_eq!(
            u.verify(&loose(&SOL), publish(&u)),
            Err(PythError::NonPositivePrice),
            "price {price}"
        );
    }
}

#[test]
fn test_verify_staleness_boundary() {
    for f in &FULL {
        let u = load(f);
        let t = publish(&u).get();
        let req = PriceRequirements::new(feed(f), Some(30), 1_000);
        assert!(u.verify(&req, UnixTimestamp::new(t + 30)).is_ok());
        assert_eq!(
            u.verify(&req, UnixTimestamp::new(t + 31)),
            Err(PythError::Stale),
            "{}",
            f.name
        );
        // A publish time ahead of the Solana clock is fresh, as in the receiver SDK.
        assert!(u.verify(&req, UnixTimestamp::new(t - 5)).is_ok());
        // No staleness bound: any age passes.
        let unbounded = PriceRequirements::new(feed(f), None, 1_000);
        assert!(u.verify(&unbounded, UnixTimestamp::new(i64::MAX)).is_ok());
        // Max age 0: only the publish second itself.
        let zero = PriceRequirements::new(feed(f), Some(0), 1_000);
        assert!(u.verify(&zero, UnixTimestamp::new(t)).is_ok());
        assert_eq!(
            u.verify(&zero, UnixTimestamp::new(t + 1)),
            Err(PythError::Stale)
        );
    }
}

#[test]
fn test_verify_staleness_extreme_times_no_overflow() {
    let d = patched(SOL.data, PUBLISH_AT, &i64::MIN.to_le_bytes());
    let u = PriceUpdate::decode(&d).unwrap();
    let req = PriceRequirements::new(feed(&SOL), Some(u64::MAX), 1_000);
    assert!(u.verify(&req, UnixTimestamp::new(i64::MAX)).is_ok());
    let req = PriceRequirements::new(feed(&SOL), Some(60), 1_000);
    assert_eq!(
        u.verify(&req, UnixTimestamp::new(i64::MAX)),
        Err(PythError::Stale)
    );
}

#[test]
fn test_verify_confidence_boundary() {
    // price 1_000_000, conf 1_000: exactly 10 bps.
    let d = patched(SOL.data, PRICE_AT, &1_000_000i64.to_le_bytes());
    let d = patched(&d, CONF_AT, &1_000u64.to_le_bytes());
    let u = PriceUpdate::decode(&d).unwrap();
    let at = |bps| u.verify(&PriceRequirements::new(feed(&SOL), None, bps), publish(&u));
    assert!(at(10).is_ok());
    assert_eq!(at(9), Err(PythError::ConfidenceTooWide));
    assert_eq!(at(0), Err(PythError::ConfidenceTooWide));
    // conf 0 passes any bound, including 0.
    let d0 = patched(&d, CONF_AT, &0u64.to_le_bytes());
    let u0 = PriceUpdate::decode(&d0).unwrap();
    assert!(u0
        .verify(&PriceRequirements::new(feed(&SOL), None, 0), publish(&u0))
        .is_ok());
    // Extreme values stay exact in u128.
    let dm = patched(&d, PRICE_AT, &i64::MAX.to_le_bytes());
    let dm = patched(&dm, CONF_AT, &u64::MAX.to_le_bytes());
    let um = PriceUpdate::decode(&dm).unwrap();
    let req = PriceRequirements::new(feed(&SOL), None, u16::MAX);
    assert!(um.verify(&req, publish(&um)).is_ok());
    let req = PriceRequirements::new(feed(&SOL), None, 20_000);
    assert_eq!(
        um.verify(&req, publish(&um)),
        Err(PythError::ConfidenceTooWide)
    );
}

#[test]
fn test_verify_real_confidence_against_ten_percent() {
    // The fixtures' real conf / price, checked against the varswap sanity bound (10%).
    for f in &FULL {
        let u = load(f);
        let m = u.message();
        assert!(
            u128::from(m.conf()) * 10_000 <= m.price() as u128 * 1_000,
            "{}",
            f.name
        );
        assert!(u
            .verify(&PriceRequirements::new(feed(f), None, 1_000), publish(&u))
            .is_ok());
    }
}

#[test]
fn test_verify_check_order_first_failure_reported() {
    // Partial, other feed, zero price, stale and wide: NotFullyVerified wins.
    let u = load(&PARTIAL);
    let req = PriceRequirements::new(feed(&SOL), Some(0), 0);
    assert_eq!(
        u.verify(&req, UnixTimestamp::new(i64::MAX)),
        Err(PythError::NotFullyVerified)
    );
    // Full, other feed and stale: WrongFeed before Stale.
    let s = load(&SOL);
    let req = PriceRequirements::new(feed(&XAU), Some(0), 0);
    assert_eq!(
        s.verify(&req, UnixTimestamp::new(i64::MAX)),
        Err(PythError::WrongFeed)
    );
    // Zero price and stale: NonPositivePrice before Stale.
    let d = patched(SOL.data, PRICE_AT, &0i64.to_le_bytes());
    let z = PriceUpdate::decode(&d).unwrap();
    let req = PriceRequirements::new(feed(&SOL), Some(0), 0);
    assert_eq!(
        z.verify(&req, UnixTimestamp::new(i64::MAX)),
        Err(PythError::NonPositivePrice)
    );
    // Stale and wide: Stale before ConfidenceTooWide.
    assert_eq!(
        s.verify(&req, UnixTimestamp::new(i64::MAX)),
        Err(PythError::Stale)
    );
}
