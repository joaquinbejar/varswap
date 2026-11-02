//! `pyth_lite::PriceUpdate::load`, `decode` and `verify` on arbitrary account bytes.
//!
//! Invariants: no panic; an accepted account re-encodes (independent encoder below) to the
//! bytes it was decoded from; a wrong owner or any changed discriminator byte is rejected;
//! `verify` accepts exactly when the reference predicate (Full, feed, positive price,
//! staleness, confidence in exact integers) holds, and then returns the message's fields.

#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use pyth_lite::{
    FeedId, PriceRequirements, PriceUpdate, PythError, UnixTimestamp, VerificationLevel,
    PRICE_UPDATE_V2_DISCRIMINATOR, RECEIVER_PROGRAM_ID,
};

/// Real mainnet `PriceUpdateV2` accounts (pyth-lite/fixtures): Full SOL/USD, Full XAG/USD
/// and a Partial one.
const FIXTURES: [&[u8]; 3] = [
    include_bytes!("../../pyth-lite/fixtures/sol_usd.data.bin"),
    include_bytes!("../../pyth-lite/fixtures/xag_usd.data.bin"),
    include_bytes!("../../pyth-lite/fixtures/partial.data.bin"),
];

#[derive(Debug, Arbitrary)]
enum Base<'a> {
    /// A real account with byte edits `(offset, xor)` (offsets past the end are ignored) and
    /// a new length (`None`: unchanged).
    Fixture {
        index: u8,
        edits: Vec<(u16, u8)>,
        len: Option<u16>,
    },
    /// Arbitrary bytes.
    Raw(&'a [u8]),
}

#[derive(Debug, Arbitrary)]
struct Input<'a> {
    /// Use the receiver as owner (otherwise `other_owner`).
    owner_is_receiver: bool,
    other_owner: [u8; 32],
    base: Base<'a>,
    /// Requirements: the message's own feed, or `other_feed`.
    same_feed: bool,
    other_feed: [u8; 32],
    max_age: Option<u64>,
    max_conf_bps: u16,
    now: i64,
}

/// The decoded fields re-encoded in the receiver's Borsh layout, up to `posted_slot`.
fn encode(u: &PriceUpdate) -> Vec<u8> {
    let m = u.message();
    let mut d = PRICE_UPDATE_V2_DISCRIMINATOR.to_vec();
    d.extend_from_slice(&u.write_authority());
    match u.verification_level() {
        VerificationLevel::Partial { num_signatures } => d.extend_from_slice(&[0, num_signatures]),
        VerificationLevel::Full => d.push(1),
    }
    d.extend_from_slice(&m.feed_id().to_bytes());
    d.extend_from_slice(&m.price().to_le_bytes());
    d.extend_from_slice(&m.conf().to_le_bytes());
    d.extend_from_slice(&m.exponent().to_le_bytes());
    d.extend_from_slice(&m.publish_time().get().to_le_bytes());
    d.extend_from_slice(&m.prev_publish_time().get().to_le_bytes());
    d.extend_from_slice(&m.ema_price().to_le_bytes());
    d.extend_from_slice(&m.ema_conf().to_le_bytes());
    d.extend_from_slice(&u.posted_slot().to_le_bytes());
    d
}

fuzz_target!(|input: Input<'_>| {
    let data = match &input.base {
        Base::Raw(b) => b.to_vec(),
        Base::Fixture { index, edits, len } => {
            let mut d = FIXTURES[usize::from(*index) % FIXTURES.len()].to_vec();
            for (at, x) in edits {
                if let Some(b) = d.get_mut(usize::from(*at)) {
                    *b ^= x;
                }
            }
            if let Some(l) = len {
                d.resize(usize::from(*l), 0);
            }
            d
        }
    };
    let owner = if input.owner_is_receiver {
        RECEIVER_PROGRAM_ID
    } else {
        input.other_owner
    };

    let loaded = PriceUpdate::load(&owner, &data, &RECEIVER_PROGRAM_ID);
    if owner != RECEIVER_PROGRAM_ID {
        assert_eq!(loaded, Err(PythError::WrongOwner));
    }
    let decoded = PriceUpdate::decode(&data);
    if owner == RECEIVER_PROGRAM_ID {
        assert_eq!(loaded, decoded);
    }
    let Ok(u) = decoded else {
        return;
    };

    // Round trip: the decoded fields are exactly the bytes they came from.
    let enc = encode(&u);
    assert_eq!(&data[..enc.len()], &enc[..]);

    // Any changed discriminator byte is rejected.
    for i in 0..8 {
        let mut bad = data.clone();
        bad[i] ^= 0x01;
        assert_eq!(
            PriceUpdate::decode(&bad),
            Err(PythError::WrongDiscriminator)
        );
    }

    // verify against the reference predicate.
    let m = u.message();
    let feed = if input.same_feed {
        m.feed_id()
    } else {
        FeedId::new(input.other_feed)
    };
    let req = PriceRequirements::new(feed, input.max_age, input.max_conf_bps);
    let got = u.verify(&req, UnixTimestamp::new(input.now));

    let full = u.verification_level() == VerificationLevel::Full;
    let feed_ok = feed == m.feed_id();
    let positive = m.price() > 0;
    let fresh = match input.max_age {
        None => true,
        Some(a) => i128::from(input.now) - i128::from(m.publish_time().get()) <= i128::from(a),
    };
    let conf_ok = u128::from(m.conf()) * 10_000
        <= u128::from(m.price().max(0).unsigned_abs()) * u128::from(input.max_conf_bps);
    let want = if !full {
        Err(PythError::NotFullyVerified)
    } else if !feed_ok {
        Err(PythError::WrongFeed)
    } else if !positive {
        Err(PythError::NonPositivePrice)
    } else if !fresh {
        Err(PythError::Stale)
    } else if !conf_ok {
        Err(PythError::ConfidenceTooWide)
    } else {
        Ok(())
    };
    match (got, want) {
        (Ok(p), Ok(())) => {
            assert_eq!(p.feed_id(), m.feed_id());
            assert_eq!(i128::from(p.price().get()), i128::from(m.price()));
            assert_eq!(p.conf(), m.conf());
            assert_eq!(p.exponent(), m.exponent());
            assert_eq!(p.publish_time(), m.publish_time());
            assert_eq!(p.prev_publish_time(), m.prev_publish_time());
        }
        (Err(g), Err(w)) => assert_eq!(g, w),
        (g, w) => panic!("verify returned {g:?}, reference {w:?}"),
    }
});
