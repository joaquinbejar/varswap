//! Realized-variance accumulator over canonical Pyth prints (`docs/PLAN.md` 2, 3.1, 3.3, 4.1,
//! 4.3).
//!
//! `no_std`, no allocator, no floating point, no `unsafe`. A [`WindowSpec`] fixes a sampling
//! grid `b_k = start + k × interval`, `k` in `[0, N]`. Each bucket `k` takes the unique Pyth
//! print with `prev_publish_time < b_k <= publish_time <= b_k + tolerance`
//! ([`WindowSpec::canonical_run`]); buckets fill in any order until `end + backfill_deadline`,
//! and a window that is still missing a bucket then degrades and its swaps unwind.
//!
//! [`Window`] is a view over the account bytes: the [`layout::WindowHeader`], a filled bitmap
//! and the per-bucket price and exponent arrays, of [`window_size`] bytes. The squared log
//! return and the annualization are behind the [`ReturnKernel`] trait, so this crate does not
//! depend on the choice of `ln` (optfix milestone 3); its tests use an exact integer kernel.

#![cfg_attr(not(test), no_std)]

mod error;
mod kernel;
pub mod layout;
mod print;
mod spec;
mod types;
mod window;

pub use error::CoreError;
pub use kernel::{ReturnKernel, SumSq};
pub use print::{Print, StoredPrice};
pub use spec::{BucketRun, RiskParams, WindowSpec};
pub use types::{Address, Bps, Seconds, WindowState, YEAR_SECONDS};
pub use window::{
    window_size, Window, CPI_ACCOUNT_CREATION_LIMIT, MAX_BUCKETS, WINDOW_TAG, WINDOW_VERSION,
};

pub use pyth_lite::{FeedId, UnixTimestamp};
