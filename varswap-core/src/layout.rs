//! Byte layouts of the window account header (`docs/PLAN.md` 3.1).
//!
//! Declarations only, as `rules/global_rules.md` requires: `derive(Pod)` expands to a
//! compile-time `const _: () = assert!(..)` (its no-padding check), which `disallowed_macros`
//! flags and an item-level `#[allow]` does not reach, so this module allows it. No functions
//! or logic live here, so the allowance never covers a runtime assertion. Validation is in
//! `spec.rs` and `window.rs`.
#![allow(clippy::disallowed_macros)]

use bytemuck::{Pod, Zeroable};

/// Common account prefix, 8 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Pod, Zeroable)]
#[repr(C)]
pub struct Hdr {
    /// Account kind; [`crate::WINDOW_TAG`] for a window, 0 while uninitialized.
    pub tag: u8,
    /// Layout version, [`crate::WINDOW_VERSION`].
    pub version: u8,
    /// PDA bump of the account.
    pub bump: u8,
    /// Explicit padding, zero.
    pub _pad: [u8; 5],
}

/// Stored form of [`crate::WindowSpec`], 96 bytes. Fields are ordered by alignment so the
/// only padding is the explicit trailing `_pad`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Pod, Zeroable)]
#[repr(C)]
pub struct WindowSpecRaw {
    /// Pyth feed id.
    pub feed: [u8; 32],
    /// `b_0`, Unix seconds.
    pub start: i64,
    /// `b_N`, Unix seconds.
    pub end: i64,
    /// Pyth receiver program that must own every sampled price update.
    pub receiver_program: [u8; 32],
    /// Grid spacing, seconds.
    pub interval: u32,
    /// Outage guard, seconds: `publish_time <= b_k + tolerance`.
    pub tolerance: u32,
    /// Buckets can be filled until `end + backfill_deadline`, seconds.
    pub backfill_deadline: u32,
    /// Confidence sanity bound, bps.
    pub max_conf: u16,
    /// Explicit padding, zero.
    pub _pad: [u8; 2],
}

/// Window account header, 144 bytes; followed by the filled bitmap
/// (`[u64; ceil(B / 64)]`), the prices (`[i64; B]`) and the exponents (`[i8; B]`), all
/// little endian, `B = N + 1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Pod, Zeroable)]
#[repr(C)]
pub struct WindowHeader {
    /// Account prefix.
    pub hdr: Hdr,
    /// The window's grid and risk parameters.
    pub spec: WindowSpecRaw,
    /// [`crate::WindowState`] as `u8`.
    pub state: u8,
    /// Explicit padding, zero.
    pub _pad: [u8; 3],
    /// Number of filled buckets.
    pub filled: u32,
    /// [`crate::SumSq`] raw value, `u128` little endian (byte array: `u128` alignment differs
    /// between host and SBF).
    pub sum_sq: [u8; 16],
    /// Annualized realized variance in the kernel's raw units, `u128` little endian; set at
    /// finalize, zero before and when degraded.
    pub realized_var: [u8; 16],
}
