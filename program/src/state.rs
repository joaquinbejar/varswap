//! Loading and validating the program's accounts, and the shared account checks.

use core::mem::size_of;

use pinocchio::cpi::Seed;
use pinocchio::{AccountView, Address, ProgramResult};
use varswap_core::layout::Hdr;
use varswap_core::{Bps, RiskParams, Seconds};

use crate::error::VarswapError;
use crate::layout::{ConfigData, FeedConfigData};
use crate::pda::{self, CONFIG_SEED, FEED_SEED};

/// Account tag of `Config` (`varswap_core::WINDOW_TAG` is 1).
pub const CONFIG_TAG: u8 = 2;
/// Account tag of `FeedConfig`.
pub const FEED_CONFIG_TAG: u8 = 3;
/// Layout version of the program's own accounts.
pub const VERSION: u8 = 1;
/// `Config` size, bytes.
pub const CONFIG_LEN: usize = size_of::<ConfigData>();
/// `FeedConfig` size, bytes.
pub const FEED_CONFIG_LEN: usize = size_of::<FeedConfigData>();

/// The account must have signed.
///
/// # Errors
///
/// [`VarswapError::MissingSignature`].
#[inline]
pub fn signer(a: &AccountView) -> ProgramResult {
    if a.is_signer() {
        Ok(())
    } else {
        Err(VarswapError::MissingSignature.into())
    }
}

/// The account must be writable.
///
/// # Errors
///
/// [`VarswapError::NotWritable`].
#[inline]
pub fn writable(a: &AccountView) -> ProgramResult {
    if a.is_writable() {
        Ok(())
    } else {
        Err(VarswapError::NotWritable.into())
    }
}

/// The account must be the system program.
///
/// # Errors
///
/// [`VarswapError::InvalidSystemProgram`].
#[inline]
pub fn system_program(a: &AccountView) -> ProgramResult {
    if *a.address() == pinocchio_system::ID {
        Ok(())
    } else {
        Err(VarswapError::InvalidSystemProgram.into())
    }
}

/// The account must be owned by this program.
///
/// # Errors
///
/// [`VarswapError::InvalidAccountOwner`].
#[inline]
pub fn owned(a: &AccountView, program_id: &Address) -> ProgramResult {
    if a.owned_by(program_id) {
        Ok(())
    } else {
        Err(VarswapError::InvalidAccountOwner.into())
    }
}

/// An account prefix for `tag`, version [`VERSION`].
#[inline]
#[must_use]
pub const fn hdr(tag: u8, bump: u8) -> Hdr {
    Hdr {
        tag,
        version: VERSION,
        bump,
        _pad: [0; 5],
    }
}

fn check_hdr(h: &Hdr, tag: u8) -> ProgramResult {
    if h.tag != tag || h.version != VERSION || h._pad != [0; 5] {
        return Err(VarswapError::InvalidAccountData.into());
    }
    Ok(())
}

/// Reads exactly one `T` from an account's data.
fn read<T: bytemuck::Pod>(a: &AccountView) -> Result<T, VarswapError> {
    let data = a
        .try_borrow()
        .map_err(|_| VarswapError::InvalidAccountData)?;
    bytemuck::try_pod_read_unaligned(&data).map_err(|_| VarswapError::InvalidAccountData)
}

/// Loads the `Config`: owner, size, tag, version, PDA `["config"]` with the stored bump.
///
/// # Errors
///
/// `InvalidAccountOwner`, `InvalidAccountData` or `InvalidPda`.
pub fn load_config(a: &AccountView, program_id: &Address) -> Result<ConfigData, VarswapError> {
    owned(a, program_id).map_err(|_| VarswapError::InvalidAccountOwner)?;
    let c: ConfigData = read(a)?;
    check_hdr(&c.hdr, CONFIG_TAG).map_err(|_| VarswapError::InvalidAccountData)?;
    pda::verify(&[CONFIG_SEED, &[c.hdr.bump]], program_id, a.address())
        .map_err(|_| VarswapError::InvalidPda)?;
    Ok(c)
}

/// Loads a `FeedConfig`: owner, size, tag, version, padding, PDA `["feed", feed]` with the
/// stored bump.
///
/// # Errors
///
/// `InvalidAccountOwner`, `InvalidAccountData` or `InvalidPda`.
pub fn load_feed_config(
    a: &AccountView,
    program_id: &Address,
) -> Result<FeedConfigData, VarswapError> {
    owned(a, program_id).map_err(|_| VarswapError::InvalidAccountOwner)?;
    let f: FeedConfigData = read(a)?;
    check_hdr(&f.hdr, FEED_CONFIG_TAG).map_err(|_| VarswapError::InvalidAccountData)?;
    if f._pad != [0; 6] {
        return Err(VarswapError::InvalidAccountData);
    }
    pda::verify(
        &[FEED_SEED, &f.feed, &[f.hdr.bump]],
        program_id,
        a.address(),
    )
    .map_err(|_| VarswapError::InvalidPda)?;
    Ok(f)
}

/// The risk parameters a window copies from its feed config (`docs/PLAN.md` 3.3).
#[inline]
#[must_use]
pub fn risk(f: &FeedConfigData) -> RiskParams {
    RiskParams {
        tolerance: Seconds::new(f.tolerance),
        max_conf: Bps::new(f.max_conf),
        backfill_deadline: Seconds::new(f.backfill_deadline),
        receiver_program: f.receiver_program,
    }
}

/// Writes `value` as the whole data of `a`.
///
/// # Errors
///
/// `InvalidAccountData` if the size differs; a borrow error.
pub fn write<T: bytemuck::Pod>(a: &mut AccountView, value: &T) -> ProgramResult {
    let mut data = a.try_borrow_mut()?;
    let bytes = bytemuck::bytes_of(value);
    if data.len() != bytes.len() {
        return Err(VarswapError::InvalidAccountData.into());
    }
    data.copy_from_slice(bytes);
    Ok(())
}

/// Seeds with a one-byte bump, as `Seed`s for a signed CPI.
#[inline]
#[must_use]
pub fn seeds<'a, const N: usize>(parts: [&'a [u8]; N]) -> [Seed<'a>; N] {
    parts.map(Seed::from)
}
