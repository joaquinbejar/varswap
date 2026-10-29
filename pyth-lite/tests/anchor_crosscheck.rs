//! Cross-check of the pyth-lite decoder against the Anchor reference,
//! `pyth_solana_receiver_sdk::price_update::PriceUpdateV2` (docs/PLAN.md 4.6), on every
//! committed mainnet fixture and on synthetic variants of each.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    missing_docs
)]

use anchor_lang::{prelude::Pubkey, AccountDeserialize, Discriminator};
use pyth_lite::{
    PriceUpdate, PythError, VerificationLevel, PRICE_UPDATE_V2_DISCRIMINATOR,
    PUSH_ORACLE_PROGRAM_ID,
};
use pyth_solana_receiver_sdk::price_update::{self as sdk, PriceUpdateV2};

const FIXTURES: [(&str, &[u8]); 4] = [
    ("xau_usd", include_bytes!("../fixtures/xau_usd.data.bin")),
    ("xag_usd", include_bytes!("../fixtures/xag_usd.data.bin")),
    ("sol_usd", include_bytes!("../fixtures/sol_usd.data.bin")),
    ("partial", include_bytes!("../fixtures/partial.data.bin")),
];

/// The three push oracle fixtures and the address `fixtures/fetch.py` fetched each from.
const PUSH_FEEDS: [(&str, &[u8], &str); 3] = [
    (
        "xau_usd",
        include_bytes!("../fixtures/xau_usd.data.bin"),
        "2uPQGpm8X4ZkxMHxrAW1QuhXcse1AHEgPih6Xp9NuEWW",
    ),
    (
        "xag_usd",
        include_bytes!("../fixtures/xag_usd.data.bin"),
        "H9JxsWwtDZxjSL6m7cdCVsWibj3JBMD9sxqLjadoZnot",
    ),
    (
        "sol_usd",
        include_bytes!("../fixtures/sol_usd.data.bin"),
        "7UVimffxr9ow1uXYxsr4LHAcV58mLzhmwaeKvJ1pjLiE",
    ),
];

fn same(name: &str, ours: &PriceUpdate, theirs: &PriceUpdateV2) {
    assert_eq!(
        ours.write_authority(),
        theirs.write_authority.to_bytes(),
        "{name}"
    );
    let level = match theirs.verification_level {
        sdk::VerificationLevel::Partial { num_signatures } => {
            VerificationLevel::Partial { num_signatures }
        }
        sdk::VerificationLevel::Full => VerificationLevel::Full,
    };
    assert_eq!(ours.verification_level(), level, "{name}");
    let (m, t) = (ours.message(), &theirs.price_message);
    assert_eq!(m.feed_id().to_bytes(), t.feed_id, "{name}");
    assert_eq!(m.price(), t.price, "{name}");
    assert_eq!(m.conf(), t.conf, "{name}");
    assert_eq!(m.exponent(), t.exponent, "{name}");
    assert_eq!(m.publish_time().get(), t.publish_time, "{name}");
    assert_eq!(m.prev_publish_time().get(), t.prev_publish_time, "{name}");
    assert_eq!(m.ema_price(), t.ema_price, "{name}");
    assert_eq!(m.ema_conf(), t.ema_conf, "{name}");
    assert_eq!(ours.posted_slot(), theirs.posted_slot, "{name}");
}

#[test]
fn test_discriminator_matches_anchor() {
    assert_eq!(PriceUpdateV2::DISCRIMINATOR, PRICE_UPDATE_V2_DISCRIMINATOR);
}

#[test]
fn test_push_feed_addresses_and_write_authority() {
    let program = Pubkey::new_from_array(PUSH_ORACLE_PROGRAM_ID);
    for (name, data, address) in PUSH_FEEDS {
        let u = PriceUpdate::decode(data).unwrap();
        let (pda, _) = Pubkey::find_program_address(
            &[&0u16.to_le_bytes(), &u.message().feed_id().to_bytes()],
            &program,
        );
        assert_eq!(pda.to_string(), address, "{name}");
        // The push oracle signs its post_update CPI with the feed PDA.
        assert_eq!(u.write_authority(), pda.to_bytes(), "{name}");
    }
}

#[test]
fn test_fixtures_decode_like_anchor() {
    for (name, data) in FIXTURES {
        let ours = PriceUpdate::decode(data).unwrap();
        let theirs = PriceUpdateV2::try_deserialize(&mut &data[..]).unwrap();
        same(name, &ours, &theirs);
    }
}

#[test]
fn test_synthetic_variants_agree_with_anchor() {
    // Every single-byte change after the discriminator, and every truncation: pyth-lite and
    // Anchor accept and reject the same inputs and decode the same fields.
    for (name, data) in FIXTURES {
        let mut variants: Vec<Vec<u8>> = (0..=data.len()).map(|n| data[..n].to_vec()).collect();
        for i in 8..data.len() {
            for x in [0x01u8, 0x80, 0xff] {
                let mut v = data.to_vec();
                v[i] ^= x;
                variants.push(v);
            }
        }
        for v in &variants {
            let ours = PriceUpdate::decode(v);
            let theirs = PriceUpdateV2::try_deserialize(&mut &v[..]);
            match (&ours, &theirs) {
                (Ok(o), Ok(t)) => same(name, o, t),
                (Err(_), Err(_)) => {}
                _ => panic!("{name}: pyth-lite {ours:?}, anchor ok = {}", theirs.is_ok()),
            }
        }
    }
}

#[test]
fn test_anchor_rejects_what_we_reject_for_tags() {
    for (name, data) in FIXTURES {
        for tag in 2u8..=255 {
            let mut v = data.to_vec();
            v[40] = tag;
            assert_eq!(
                PriceUpdate::decode(&v),
                Err(PythError::UnknownVerificationLevel)
            );
            assert!(
                PriceUpdateV2::try_deserialize(&mut &v[..]).is_err(),
                "{name} tag {tag}"
            );
        }
    }
}
