//! Program-derived addresses, with the program's seeds (`program/src/pda.rs`).

use crate::{Address, ATA_PROGRAM, TOKEN_PROGRAM, UPGRADEABLE_LOADER};

/// The terms that name a window (its PDA seeds).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowKey {
    /// Pyth feed id.
    pub feed: [u8; 32],
    /// `b_0`, Unix seconds.
    pub start: i64,
    /// `b_N`, Unix seconds.
    pub end: i64,
    /// Grid spacing, seconds.
    pub interval: u32,
}

/// `["config"]`.
#[must_use]
pub fn config(program_id: &Address) -> (Address, u8) {
    Address::find_program_address(&[b"config"], program_id)
}

/// `["feed", feed_id]`.
#[must_use]
pub fn feed_config(program_id: &Address, feed: &[u8; 32]) -> (Address, u8) {
    Address::find_program_address(&[b"feed", feed], program_id)
}

/// `["window", feed, start, end, interval]`, little endian.
#[must_use]
pub fn window(program_id: &Address, w: &WindowKey) -> (Address, u8) {
    Address::find_program_address(
        &[
            b"window",
            &w.feed,
            &w.start.to_le_bytes(),
            &w.end.to_le_bytes(),
            &w.interval.to_le_bytes(),
        ],
        program_id,
    )
}

/// `["swap", window, long, short, swap_index]`, little endian.
#[must_use]
pub fn swap(
    program_id: &Address,
    window: &Address,
    long: &Address,
    short: &Address,
    index: u64,
) -> (Address, u8) {
    Address::find_program_address(
        &[
            b"swap",
            window.as_ref(),
            long.as_ref(),
            short.as_ref(),
            &index.to_le_bytes(),
        ],
        program_id,
    )
}

/// `["vault", swap]`: the swap's collateral token account.
#[must_use]
pub fn vault(program_id: &Address, swap: &Address) -> (Address, u8) {
    Address::find_program_address(&[b"vault", swap.as_ref()], program_id)
}

/// The SPL Token associated token account of `owner` for `mint`.
#[must_use]
pub fn ata(owner: &Address, mint: &Address) -> Address {
    Address::find_program_address(
        &[owner.as_ref(), TOKEN_PROGRAM.as_ref(), mint.as_ref()],
        &ATA_PROGRAM,
    )
    .0
}

/// The program's `ProgramData` account under the upgradeable loader.
#[must_use]
pub fn program_data(program_id: &Address) -> (Address, u8) {
    Address::find_program_address(&[program_id.as_ref()], &UPGRADEABLE_LOADER)
}
