//! Posts Pyth prices on Solana from Hermes updates: the Pythnet accumulator update (`PNAU`)
//! parser, the Wormhole encoded VAA and Pyth receiver instruction builders, and a poster over
//! any [`Transport`] that turns one price message into a `Full` `PriceUpdateV2`, runs the
//! caller's instructions on it and closes every account it opened, in a fixed sequence of
//! transactions ([`post_plan`]).
//!
//! Off chain, no Anchor: layouts are written out from the Pyth and Wormhole sources (cited in
//! [`ix`] and [`accumulator`]) and cross-checked against the deployed programs in LiteSVM.
//! [`RpcTransport`] is a [`Transport`] over `solana-rpc-client` that never shows its URL in
//! `Debug` or errors ([`redact`]).
//!
//! A failed post can leave its encoded VAA open: [`post`] reports it ([`PostError::Orphaned`],
//! [`PostError::Uncertain`]) and [`close_if_open`] closes it later. [`LeftoverScanner`] finds
//! any that were missed.
//!
//! **Contract: the payer is exclusive to one poster process.** [`LeftoverScanner`] and any
//! sweep built on it close the payer's encoded VAAs, and a close cannot be atomic against
//! another process's verify, so pyth-post gives no guarantee across processes sharing a payer.
//! Hold a [`PayerLock`] for the life of the process to enforce it; the scanner's
//! [`LEFTOVER_MIN_AGE`] guard is only defense in depth.
//!
//! ```no_run
//! # fn main() -> Result<(), pyth_post::PostError> {
//! # let (bytes, feed, payer): (Vec<u8>, [u8; 32], solana_keypair::Keypair) = unimplemented!();
//! use pyth_post::{post, AccumulatorUpdate, PostError, RpcTransport};
//!
//! let mut cluster = RpcTransport::new("https://api.devnet.solana.com");
//! let update = AccumulatorUpdate::parse(&bytes)?;
//! let (message, price) = update.find(&feed).ok_or(PostError::Update("no such feed"))?;
//! // `consume` builds the instructions that read the fresh PriceUpdateV2 account.
//! let signatures = post(&mut cluster, &payer, &update, message, 0, |_price_update| vec![])?;
//! # let _ = (price, signatures);
//! # Ok(())
//! # }
//! ```

pub mod accumulator;
mod error;
pub mod ix;
mod lock;
mod post;
pub mod redact;
mod rpc;

pub use accumulator::{price_feed_message, AccumulatorUpdate, MerkleUpdate, PriceFeedMessage};
pub use error::PostError;
pub use lock::{default_lock_dir, PayerLock, LOCK_DIR_VAR};
pub use post::{
    close_if_open, post, post_plan, scan_encoded_vaas, Extra, LeftoverScanner, PostAccounts, Step,
    Transport, UpdateSource, CLOSE_ATTEMPTS, FIRST_CHUNK, LEFTOVER_MIN_AGE, NEXT_CHUNK,
    VERIFY_COMPUTE_UNITS,
};
pub use rpc::{client_error, RpcTransport};
