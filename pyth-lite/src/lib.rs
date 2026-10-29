//! `no_std` reader for Pyth `PriceUpdateV2` accounts, written for Pinocchio programs.
//!
//! No Anchor, no allocator, no `unsafe` and no Solana dependency: the caller passes the
//! account's owner and data as bytes. [`PriceUpdate::load`] checks the owner and the Anchor
//! discriminator and decodes the Borsh layout by offset; [`PriceUpdate::verify`] then runs the
//! price checks (`Full` verification, feed id, positive price, staleness, confidence) and
//! returns an [`OraclePrice`].
//!
//! Layout (pyth-solana-receiver-sdk 2.0.0, `src/price_update.rs`): 8-byte discriminator,
//! `write_authority: [u8; 32]`, `verification_level` (`Partial { num_signatures: u8 }` is 2
//! bytes, `Full` is 1, so every later offset is computed), `price_message` (`feed_id`,
//! `price: i64`, `conf: u64`, `exponent: i32`, `publish_time: i64`, `prev_publish_time: i64`,
//! `ema_price: i64`, `ema_conf: u64`), `posted_slot: u64`. Accounts are allocated at
//! [`PRICE_UPDATE_V2_LEN`] bytes; a `Full` update leaves one trailing byte unused.

#![cfg_attr(not(test), no_std)]

mod error;
mod ids;
mod update;

pub use error::PythError;
pub use ids::{
    FeedId, PRICE_UPDATE_V2_DISCRIMINATOR, PRICE_UPDATE_V2_LEN, PUSH_ORACLE_PROGRAM_ID,
    RECEIVER_PROGRAM_ID,
};
pub use update::{
    OraclePrice, PriceMessage, PriceRequirements, PriceUpdate, UnixTimestamp, VerificationLevel,
};
