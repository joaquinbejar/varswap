//! Byte layouts of the program's own accounts and instruction payloads.
//!
//! Declarations only, as `rules/global_rules.md` requires: `derive(Pod)` expands to a
//! compile-time `const _: () = assert!(..)` (its no-padding check), which `disallowed_macros`
//! flags and an item-level `#[allow]` does not reach, so this module allows it. No functions
//! or logic live here. The window layout is `varswap_core::layout`.
#![allow(clippy::disallowed_macros)]

use bytemuck::{Pod, Zeroable};
use varswap_core::layout::Hdr;

/// `Config` account at PDA `["config"]`, 72 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Pod, Zeroable)]
#[repr(C)]
pub struct ConfigData {
    /// Prefix; tag [`crate::state::CONFIG_TAG`].
    pub hdr: Hdr,
    /// The only signer allowed to write feed configs.
    pub admin: [u8; 32],
    /// The only mint swaps collateralize in (USDC on mainnet).
    pub collateral_mint: [u8; 32],
}

/// `init_config` payload, 32 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Pod, Zeroable)]
#[repr(C)]
pub struct InitConfigArgs {
    /// The collateral mint, nonzero.
    pub collateral_mint: [u8; 32],
}

/// `Swap` account at PDA `["swap", window, long, short, swap_index_le]`, 288 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Pod, Zeroable)]
#[repr(C)]
pub struct SwapData {
    /// Prefix; tag [`crate::state::SWAP_TAG`], bump of the swap PDA.
    pub hdr: Hdr,
    /// The window the swap settles on.
    pub window: [u8; 32],
    /// The long party (receives `realized − strike`).
    pub long: [u8; 32],
    /// The short party.
    pub short: [u8; 32],
    /// Collateral mint, copied from `Config` at opening.
    pub mint: [u8; 32],
    /// Token program, copied at opening (SPL Token).
    pub token_program: [u8; 32],
    /// Who paid the rent; receives it back at `close_swap`.
    pub payer: [u8; 32],
    /// Atoms per 1.0 of annualized variance.
    pub notional: u64,
    /// Disambiguates several swaps between the same parties on the same window.
    pub swap_index: u64,
    /// Long collateral, atoms.
    pub long_collateral: u64,
    /// Short collateral, atoms.
    pub short_collateral: u64,
    /// Long payout, atoms, set at settlement.
    pub long_payout: u64,
    /// Short payout, atoms, set at settlement.
    pub short_payout: u64,
    /// Strike variance, Q64.64 raw `u128` little endian.
    pub strike_var: [u8; 16],
    /// Cap variance, Q64.64 raw `u128` little endian.
    pub cap_var: [u8; 16],
    /// [`crate::state::SwapState`] as `u8`.
    pub state: u8,
    /// 1 once the long's payout was transferred.
    pub long_claimed: u8,
    /// 1 once the short's payout was transferred.
    pub short_claimed: u8,
    /// Bump of the `["vault", swap]` PDA.
    pub vault_bump: u8,
    /// Explicit padding, zero.
    pub _pad: [u8; 4],
}

/// `open_swap` payload, 48 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Pod, Zeroable)]
#[repr(C)]
pub struct OpenSwapArgs {
    /// Atoms per 1.0 of annualized variance.
    pub notional: u64,
    /// Swap index for the PDA.
    pub swap_index: u64,
    /// Strike variance, Q64.64 raw `u128` little endian.
    pub strike_var: [u8; 16],
    /// Cap variance, Q64.64 raw `u128` little endian.
    pub cap_var: [u8; 16],
}

/// `FeedConfig` account at PDA `["feed", feed_id]`, 96 bytes: the admin's risk parameters for
/// one feed, copied by every window created on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Pod, Zeroable)]
#[repr(C)]
pub struct FeedConfigData {
    /// Prefix; tag [`crate::state::FEED_CONFIG_TAG`].
    pub hdr: Hdr,
    /// Pyth feed id.
    pub feed: [u8; 32],
    /// Pyth receiver program that must own sampled price updates.
    pub receiver_program: [u8; 32],
    /// Smallest window interval allowed on this feed, seconds.
    pub min_interval: u32,
    /// Outage guard, seconds.
    pub tolerance: u32,
    /// Backfill period after a window's end, seconds.
    pub backfill_deadline: u32,
    /// Largest `N + 1` of a window on this feed, at most `MAX_BUCKETS`.
    pub max_buckets: u32,
    /// Confidence sanity bound, bps.
    pub max_conf: u16,
    /// The feed's Pyth exponent, copied by windows; prints with another one are rejected.
    pub exponent: i8,
    /// Explicit padding, zero.
    pub _pad: [u8; 5],
}

/// `set_feed_config` payload, 84 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Pod, Zeroable)]
#[repr(C)]
pub struct SetFeedConfigArgs {
    /// Pyth feed id.
    pub feed: [u8; 32],
    /// Pyth receiver program id.
    pub receiver_program: [u8; 32],
    /// Smallest window interval, seconds, `> 0`.
    pub min_interval: u32,
    /// Outage guard, seconds, `> 0`.
    pub tolerance: u32,
    /// Backfill period, seconds, `> 0`.
    pub backfill_deadline: u32,
    /// Largest `N + 1`, in `[2, MAX_BUCKETS]`.
    pub max_buckets: u32,
    /// Confidence bound, bps, in `(0, 10_000]`.
    pub max_conf: u16,
    /// The feed's Pyth exponent.
    pub exponent: i8,
    /// Explicit padding, zero.
    pub _pad: [u8; 1],
}

/// `create_window` payload, 24 bytes. The feed comes from the `FeedConfig` account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Pod, Zeroable)]
#[repr(C)]
pub struct CreateWindowArgs {
    /// `b_0`, Unix seconds.
    pub start: i64,
    /// `b_N`, Unix seconds.
    pub end: i64,
    /// Grid spacing, seconds.
    pub interval: u32,
    /// Explicit padding, zero.
    pub _pad: [u8; 4],
}

/// `sample` return data, 24 bytes: the bucket run filled and the print that filled it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Pod, Zeroable)]
#[repr(C)]
pub struct SampleResult {
    /// Print price mantissa.
    pub price: i64,
    /// Print `publish_time`, Unix seconds.
    pub publish_time: i64,
    /// First bucket filled.
    pub first: u32,
    /// Last bucket filled, inclusive.
    pub last: u32,
}
