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
//! return and the annualization are behind the [`ReturnKernel`] trait: [`LnKernel`] (optfix
//! `ln`, Q64.64) is the production kernel, and [`IntegerKernel`] an exact stand-in that the
//! accumulator tests use to compare windows bit for bit.

#![cfg_attr(not(test), no_std)]

mod error;
mod kernel;
pub mod layout;
mod print;
mod spec;
mod types;
mod window;

pub use error::CoreError;
#[cfg(feature = "insecure-test-kernel")]
pub use kernel::IntegerKernel;
pub use kernel::{LnKernel, ReturnKernel, SumSq, LN_10_RAW};
pub use print::{Print, StoredPrice};
pub use spec::{BucketRun, RiskParams, WindowSpec};
pub use types::{Address, Bps, Seconds, WindowState, YEAR_SECONDS};
pub use window::{
    window_size, Window, CPI_ACCOUNT_CREATION_LIMIT, MAX_BUCKETS, WINDOW_TAG, WINDOW_VERSION,
};

pub use pyth_lite::{FeedId, UnixTimestamp};
