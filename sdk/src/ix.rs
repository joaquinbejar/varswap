//! Instruction builders, one per program instruction (tags in `program/src/lib.rs`).

use solana_instruction::{AccountMeta, Instruction};

use crate::pda::{self, WindowKey};
use crate::{Address, SYSTEM_PROGRAM, TOKEN_PROGRAM};

/// Instruction tags.
pub mod tag {
    /// `init_config`.
    pub const INIT_CONFIG: u8 = 0;
    /// `set_feed_config`.
    pub const SET_FEED_CONFIG: u8 = 1;
    /// `create_window`.
    pub const CREATE_WINDOW: u8 = 2;
    /// `sample`.
    pub const SAMPLE: u8 = 3;
    /// `finalize`.
    pub const FINALIZE: u8 = 4;
    /// `open_swap`.
    pub const OPEN_SWAP: u8 = 5;
    /// `settle_swap`.
    pub const SETTLE_SWAP: u8 = 6;
    /// `claim_long`.
    pub const CLAIM_LONG: u8 = 7;
    /// `claim_short`.
    pub const CLAIM_SHORT: u8 = 8;
    /// `close_swap`.
    pub const CLOSE_SWAP: u8 = 9;
}

/// The admin's risk parameters for one feed (`set_feed_config`, 84 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeedParams {
    /// Pyth feed id.
    pub feed: [u8; 32],
    /// Pyth receiver program that must own sampled price updates.
    pub receiver_program: Address,
    /// Smallest window interval, seconds.
    pub min_interval: u32,
    /// Outage guard, seconds.
    pub tolerance: u32,
    /// Backfill period after a window's end, seconds.
    pub backfill_deadline: u32,
    /// Largest `N + 1` of a window.
    pub max_buckets: u32,
    /// Confidence sanity bound, bps.
    pub max_conf: u16,
    /// The feed's pinned Pyth exponent.
    pub exponent: i8,
}

impl FeedParams {
    /// The 84-byte little-endian encoding.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut d = Vec::with_capacity(84);
        d.extend_from_slice(&self.feed);
        d.extend_from_slice(self.receiver_program.as_ref());
        d.extend_from_slice(&self.min_interval.to_le_bytes());
        d.extend_from_slice(&self.tolerance.to_le_bytes());
        d.extend_from_slice(&self.backfill_deadline.to_le_bytes());
        d.extend_from_slice(&self.max_buckets.to_le_bytes());
        d.extend_from_slice(&self.max_conf.to_le_bytes());
        d.extend_from_slice(&self.exponent.to_le_bytes());
        d.push(0);
        d
    }
}

/// The terms and accounts of one `open_swap`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenSwap {
    /// The window the swap settles on.
    pub window: Address,
    /// The long party (signs).
    pub long: Address,
    /// The short party (signs).
    pub short: Address,
    /// Pays both rents (signs).
    pub payer: Address,
    /// The long's collateral token account.
    pub long_src: Address,
    /// The short's collateral token account.
    pub short_src: Address,
    /// The collateral mint (the config's).
    pub mint: Address,
    /// Atoms per 1.0 of annualized variance.
    pub notional: u64,
    /// Strike variance, Q64.64 raw.
    pub strike_var: u128,
    /// Cap variance, Q64.64 raw.
    pub cap_var: u128,
    /// Swap index for the PDA.
    pub index: u64,
}

fn ix(program_id: &Address, accounts: Vec<AccountMeta>, data: Vec<u8>) -> Instruction {
    Instruction {
        program_id: *program_id,
        accounts,
        data,
    }
}

fn w(a: Address) -> AccountMeta {
    AccountMeta::new(a, false)
}

fn r(a: Address) -> AccountMeta {
    AccountMeta::new_readonly(a, false)
}

/// `init_config`: signed by the program's upgrade authority (`admin`) and the payer; sets the
/// immutable collateral mint.
#[must_use]
pub fn init_config(
    program_id: &Address,
    admin: &Address,
    payer: &Address,
    collateral_mint: &Address,
) -> Instruction {
    let mut data = vec![tag::INIT_CONFIG];
    data.extend_from_slice(collateral_mint.as_ref());
    ix(
        program_id,
        vec![
            AccountMeta::new_readonly(*admin, true),
            AccountMeta::new(*payer, true),
            w(pda::config(program_id).0),
            r(pda::program_data(program_id).0),
            r(SYSTEM_PROGRAM),
        ],
        data,
    )
}

/// `set_feed_config`: signed by the admin; the payer pays the rent on creation.
#[must_use]
pub fn set_feed_config(
    program_id: &Address,
    admin: &Address,
    payer: &Address,
    p: &FeedParams,
) -> Instruction {
    let mut data = vec![tag::SET_FEED_CONFIG];
    data.extend_from_slice(&p.encode());
    ix(
        program_id,
        vec![
            AccountMeta::new_readonly(*admin, true),
            AccountMeta::new(*payer, true),
            r(pda::config(program_id).0),
            w(pda::feed_config(program_id, &p.feed).0),
            r(SYSTEM_PROGRAM),
        ],
        data,
    )
}

/// `create_window`: anyone, paying the window's rent.
#[must_use]
pub fn create_window(program_id: &Address, payer: &Address, k: &WindowKey) -> Instruction {
    let mut data = vec![tag::CREATE_WINDOW];
    data.extend_from_slice(&k.start.to_le_bytes());
    data.extend_from_slice(&k.end.to_le_bytes());
    data.extend_from_slice(&k.interval.to_le_bytes());
    data.extend_from_slice(&[0; 4]);
    ix(
        program_id,
        vec![
            AccountMeta::new(*payer, true),
            r(pda::feed_config(program_id, &k.feed).0),
            w(pda::window(program_id, k).0),
            r(SYSTEM_PROGRAM),
        ],
        data,
    )
}

/// `sample`: anyone, with a Pyth `PriceUpdateV2` account owned by the receiver.
#[must_use]
pub fn sample(program_id: &Address, window: &Address, price_update: &Address) -> Instruction {
    ix(
        program_id,
        vec![w(*window), r(*price_update)],
        vec![tag::SAMPLE],
    )
}

/// `finalize`: anyone.
#[must_use]
pub fn finalize(program_id: &Address, window: &Address) -> Instruction {
    ix(program_id, vec![w(*window)], vec![tag::FINALIZE])
}

/// `open_swap`: signed by both parties and the payer.
#[must_use]
pub fn open_swap(program_id: &Address, o: &OpenSwap) -> Instruction {
    let swap = pda::swap(program_id, &o.window, &o.long, &o.short, o.index).0;
    let mut data = vec![tag::OPEN_SWAP];
    data.extend_from_slice(&o.notional.to_le_bytes());
    data.extend_from_slice(&o.index.to_le_bytes());
    data.extend_from_slice(&o.strike_var.to_le_bytes());
    data.extend_from_slice(&o.cap_var.to_le_bytes());
    ix(
        program_id,
        vec![
            AccountMeta::new_readonly(o.long, true),
            AccountMeta::new_readonly(o.short, true),
            AccountMeta::new(o.payer, true),
            r(pda::config(program_id).0),
            r(o.window),
            w(swap),
            w(pda::vault(program_id, &swap).0),
            w(o.long_src),
            w(o.short_src),
            r(o.mint),
            r(TOKEN_PROGRAM),
            r(SYSTEM_PROGRAM),
        ],
        data,
    )
}

/// `settle_swap`: anyone, once the window is final.
#[must_use]
pub fn settle_swap(program_id: &Address, window: &Address, swap: &Address) -> Instruction {
    ix(
        program_id,
        vec![r(*window), w(*swap)],
        vec![tag::SETTLE_SWAP],
    )
}

fn claim(
    program_id: &Address,
    t: u8,
    swap: &Address,
    dest: &Address,
    mint: &Address,
) -> Instruction {
    ix(
        program_id,
        vec![
            w(*swap),
            w(pda::vault(program_id, swap).0),
            w(*dest),
            r(*mint),
            r(TOKEN_PROGRAM),
        ],
        vec![t],
    )
}

/// `claim_long`: anyone, into the long's token account of the swap's mint.
#[must_use]
pub fn claim_long(
    program_id: &Address,
    swap: &Address,
    dest: &Address,
    mint: &Address,
) -> Instruction {
    claim(program_id, tag::CLAIM_LONG, swap, dest, mint)
}

/// `claim_short`: anyone, into the short's token account of the swap's mint.
#[must_use]
pub fn claim_short(
    program_id: &Address,
    swap: &Address,
    dest: &Address,
    mint: &Address,
) -> Instruction {
    claim(program_id, tag::CLAIM_SHORT, swap, dest, mint)
}

/// `close_swap`: anyone, after both claims; rents and any vault balance go to the payer
/// recorded in the swap (`receiver`, with `receiver_token` its token account of `mint`).
#[must_use]
pub fn close_swap(
    program_id: &Address,
    swap: &Address,
    receiver: &Address,
    receiver_token: &Address,
    mint: &Address,
) -> Instruction {
    ix(
        program_id,
        vec![
            w(*swap),
            w(pda::vault(program_id, swap).0),
            w(*receiver),
            w(*receiver_token),
            r(*mint),
            r(TOKEN_PROGRAM),
        ],
        vec![tag::CLOSE_SWAP],
    )
}
