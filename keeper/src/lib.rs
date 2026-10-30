//! Off-chain sampling keeper for varswap (`docs/PLAN.md` 4.1, milestone 6).
//!
//! A window's contract is the sum of squared log returns between the canonical prints of its
//! grid boundaries; anyone may post them. [`Keeper::tick`] does it for one window:
//!
//! 1. reads the window and decodes it with the program's own validation
//!    (`varswap_sdk::decode`, `varswap_core::Window`);
//! 2. for every empty bucket whose boundary has passed, asks the [`PriceSource`] for the
//!    canonical print (`prev_publish_time < b_k <= publish_time <= b_k + tolerance`) and posts
//!    it if the window's rules accept it (pinned exponent, positive price, confidence bound,
//!    a previous aggregate), marking the whole run it fills;
//! 3. finalizes once every bucket is filled, or once the backfill deadline has passed.
//!
//! Prices come from a [`PriceSource`] (Hermes in production, synthetic tapes in tests) and
//! transactions go through a [`Chain`] (an RPC client in production, LiteSVM in tests). A
//! failed post is benign only if the window, re-read, already holds that bucket (someone else
//! posted the same canonical print); otherwise the tick returns the error without finalizing.
//!
//! In production the [`Chain`] is a [`ReceiverChain`] over an [`RpcTransport`]: each print is
//! posted through Wormhole and the Pyth receiver as a `Full` `PriceUpdateV2` (`pyth-post`),
//! sampled and closed in the same transaction.
//!
//! The Hermes API key is read only from the environment ([`HermesConfig::from_env`]), never
//! from a file, and never printed (the metalbook-keeper pattern).

mod chain;
mod error;
mod hermes;
mod keeper;
mod receiver_chain;
mod source;

pub use chain::Chain;
pub use error::KeeperError;
pub use hermes::{
    parse_update, AuthScheme, HermesConfig, HermesSource, HermesUpdate, HttpGet, HttpResponse,
    UreqHttp, DEFAULT_HERMES_URL, HERMES_API_KEY_VAR, HERMES_AUTH_VAR, HERMES_URL_VAR,
};
pub use keeper::{Keeper, TickReport};
pub use pyth_post::{PostError, RpcTransport, Transport, UpdateSource};
pub use receiver_chain::{Orphan, ReceiverChain};
pub use source::{PriceSource, PrintMsg, TapeSource};
