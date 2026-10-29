//! A function whose SBF stack frame exceeds 4 KiB, so the stack check must fail on it.

#![no_std]
#![allow(clippy::indexing_slicing, clippy::arithmetic_side_effects)]

/// Keeps a 6,000-byte array live on the stack.
#[inline(never)]
pub fn oversized_frame(x: u8) -> u8 {
    let mut a = [0u8; 6000];
    let p = core::hint::black_box(&mut a);
    p[usize::from(x) % 6000] = x;
    core::hint::black_box(&*p)[(usize::from(x) + 7) % 6000]
}
