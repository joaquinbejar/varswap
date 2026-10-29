//! Account decoders: the owner must be the program, then the shared layouts check size, tag and
//! version (any alignment).

use varswap_core::layout::{
    ConfigData, FeedConfigData, SwapData, ACCOUNT_VERSION, CONFIG_TAG, FEED_CONFIG_TAG, SWAP_TAG,
};
use varswap_core::{CoreError, Window, WindowInfo};

use crate::Address;

/// Why an account could not be decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// Not owned by the program.
    WrongOwner,
    /// Wrong size.
    WrongSize,
    /// Wrong tag or version.
    WrongTag,
    /// A window that fails `varswap_core::Window::inspect`.
    Window(CoreError),
}

fn read<T: bytemuck::Pod>(
    program_id: &Address,
    owner: &Address,
    data: &[u8],
    tag: u8,
) -> Result<T, DecodeError> {
    if owner != program_id {
        return Err(DecodeError::WrongOwner);
    }
    let v: T = bytemuck::try_pod_read_unaligned(data).map_err(|_| DecodeError::WrongSize)?;
    // Every account starts with `Hdr { tag, version, .. }`.
    match data.get(..2) {
        Some([t, ver]) if *t == tag && *ver == ACCOUNT_VERSION => Ok(v),
        _ => Err(DecodeError::WrongTag),
    }
}

/// Decodes the config.
///
/// # Errors
///
/// [`DecodeError`].
pub fn config(
    program_id: &Address,
    owner: &Address,
    data: &[u8],
) -> Result<ConfigData, DecodeError> {
    read(program_id, owner, data, CONFIG_TAG)
}

/// Decodes a feed config.
///
/// # Errors
///
/// [`DecodeError`].
pub fn feed_config(
    program_id: &Address,
    owner: &Address,
    data: &[u8],
) -> Result<FeedConfigData, DecodeError> {
    read(program_id, owner, data, FEED_CONFIG_TAG)
}

/// Decodes a swap.
///
/// # Errors
///
/// [`DecodeError`].
pub fn swap(program_id: &Address, owner: &Address, data: &[u8]) -> Result<SwapData, DecodeError> {
    read(program_id, owner, data, SWAP_TAG)
}

/// Decodes a window's spec, state and realized variance, with the program's own validation.
///
/// # Errors
///
/// [`DecodeError`].
pub fn window(
    program_id: &Address,
    owner: &Address,
    data: &[u8],
) -> Result<WindowInfo, DecodeError> {
    if owner != program_id {
        return Err(DecodeError::WrongOwner);
    }
    Window::inspect(data).map_err(DecodeError::Window)
}

/// Which buckets of a window are filled (bit `k` of the result is bucket `k`), with the
/// program's validation; for keepers that fill the empty ones.
///
/// # Errors
///
/// [`DecodeError`].
pub fn filled_buckets(
    program_id: &Address,
    owner: &Address,
    data: &[u8],
) -> Result<Vec<bool>, DecodeError> {
    let info = window(program_id, owner, data)?;
    let buckets = info.spec.buckets().map_err(DecodeError::Window)?;
    let mut copy = data.to_vec();
    let w = Window::load(&mut copy).map_err(DecodeError::Window)?;
    (0..buckets)
        .map(|k| w.is_filled(k).map_err(DecodeError::Window))
        .collect()
}
