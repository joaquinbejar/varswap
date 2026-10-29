//! Instruction handlers. Each validates every account it receives before using it
//! (`docs/PLAN.md` 3.3) and writes state last.

use pinocchio::cpi::set_return_data;
use pinocchio::sysvars::clock::Clock;
use pinocchio::sysvars::Sysvar;
use pinocchio::{AccountView, Address, ProgramResult};
use pyth_lite::{PriceRequirements, PriceUpdate};
use varswap_core::{window_size, Print, Seconds, UnixTimestamp, Window, WindowSpec, MAX_BUCKETS};

use crate::error::{core, pyth, VarswapError};
use crate::layout::{
    ConfigData, CreateWindowArgs, FeedConfigData, SampleResult, SetFeedConfigArgs,
};
use crate::pda::{self, CONFIG_SEED, FEED_SEED, WINDOW_SEED};
use crate::state::{
    self, hdr, owned, seeds, signer, system_program, writable, CONFIG_LEN, CONFIG_TAG,
    FEED_CONFIG_LEN, FEED_CONFIG_TAG,
};
use crate::{loader, Kernel};

/// Decodes a payload of exactly `size_of::<T>()` bytes.
fn args<T: bytemuck::Pod>(payload: &[u8]) -> Result<T, VarswapError> {
    bytemuck::try_pod_read_unaligned(payload).map_err(|_| VarswapError::InvalidInstructionData)
}

fn no_args(payload: &[u8]) -> ProgramResult {
    if payload.is_empty() {
        Ok(())
    } else {
        Err(VarswapError::InvalidInstructionData.into())
    }
}

fn now() -> Result<UnixTimestamp, pinocchio::error::ProgramError> {
    Ok(UnixTimestamp::new(Clock::get()?.unix_timestamp))
}

/// `init_config`: creates `Config` at `["config"]` with `admin`, who must be the program's
/// upgrade authority, so the first caller after deployment cannot take the program over.
///
/// Accounts: admin (signer), payer (signer, writable), config (writable), program data,
/// system program.
///
/// # Errors
///
/// Missing signature, not writable, wrong system program, the `ProgramData` gate
/// ([`loader::check_upgrade_authority`]), wrong config PDA, or an existing config.
#[inline(never)]
pub fn init_config(
    program_id: &Address,
    accounts: &mut [AccountView],
    payload: &[u8],
) -> ProgramResult {
    no_args(payload)?;
    let [admin, payer, config, program_data, system, ..] = accounts else {
        return Err(VarswapError::NotEnoughAccounts.into());
    };
    signer(admin)?;
    signer(payer)?;
    writable(payer)?;
    writable(config)?;
    system_program(system)?;
    loader::check_upgrade_authority(program_id, program_data, admin.address())?;
    let (expected, bump) = pda::find(&[CONFIG_SEED], program_id)?;
    if *config.address() != expected {
        return Err(VarswapError::InvalidPda.into());
    }
    let bump_seed = [bump];
    pda::create(
        config,
        CONFIG_LEN,
        program_id,
        payer,
        &seeds([CONFIG_SEED, &bump_seed]),
    )?;
    state::write(
        config,
        &ConfigData {
            hdr: hdr(CONFIG_TAG, bump),
            admin: admin.address().to_bytes(),
        },
    )
}

/// `set_feed_config`: creates or overwrites the `FeedConfig` of a feed. Only the config
/// admin. Windows already created keep their own copy (`docs/PLAN.md` 4).
///
/// Accounts: admin (signer), payer (signer, writable), config, feed config (writable),
/// system program.
///
/// # Errors
///
/// Missing signature, not writable, wrong system program, a forged config, another admin,
/// parameters out of range, a wrong feed config PDA or owner.
#[inline(never)]
pub fn set_feed_config(
    program_id: &Address,
    accounts: &mut [AccountView],
    payload: &[u8],
) -> ProgramResult {
    let a: SetFeedConfigArgs = args(payload)?;
    let [admin, payer, config, feed_config, system, ..] = accounts else {
        return Err(VarswapError::NotEnoughAccounts.into());
    };
    signer(admin)?;
    writable(feed_config)?;
    let c = state::load_config(config, program_id)?;
    if c.admin != admin.address().to_bytes() {
        return Err(VarswapError::NotAuthority.into());
    }
    if a._pad != [0; 2] {
        return Err(VarswapError::InvalidInstructionData.into());
    }
    let new = FeedConfigData {
        hdr: hdr(FEED_CONFIG_TAG, 0),
        feed: a.feed,
        receiver_program: a.receiver_program,
        min_interval: a.min_interval,
        tolerance: a.tolerance,
        backfill_deadline: a.backfill_deadline,
        max_buckets: a.max_buckets,
        max_conf: a.max_conf,
        _pad: [0; 6],
    };
    state::risk(&new).check().map_err(core)?;
    if a.min_interval == 0 || a.max_buckets < 2 || a.max_buckets > MAX_BUCKETS {
        return Err(VarswapError::InvalidFeedConfig.into());
    }
    if a.receiver_program == [0; 32] {
        return Err(VarswapError::InvalidFeedConfig.into());
    }

    let bump = if feed_config.owned_by(program_id) {
        let old = state::load_feed_config(feed_config, program_id)?;
        if old.feed != a.feed {
            return Err(VarswapError::InvalidPda.into());
        }
        old.hdr.bump
    } else {
        signer(payer)?;
        writable(payer)?;
        system_program(system)?;
        let (expected, bump) = pda::find(&[FEED_SEED, &a.feed], program_id)?;
        if *feed_config.address() != expected {
            return Err(VarswapError::InvalidPda.into());
        }
        let bump_seed = [bump];
        pda::create(
            feed_config,
            FEED_CONFIG_LEN,
            program_id,
            payer,
            &seeds([FEED_SEED, &a.feed, &bump_seed]),
        )?;
        bump
    };
    state::write(
        feed_config,
        &FeedConfigData {
            hdr: hdr(FEED_CONFIG_TAG, bump),
            ..new
        },
    )
}

/// `create_window`: anyone creates the window `["window", feed, start, end, interval]` on a
/// vetted feed, with the feed's risk parameters (never the creator's).
///
/// Accounts: payer (signer, writable), feed config, window (writable), system program.
///
/// # Errors
///
/// Missing signature, not writable, wrong system program, a forged feed config, spec rules
/// (`varswap_core::WindowSpec::try_new`, `check_creatable`), an interval below the feed's
/// minimum, a wrong window PDA, or an existing window.
#[inline(never)]
pub fn create_window(
    program_id: &Address,
    accounts: &mut [AccountView],
    payload: &[u8],
) -> ProgramResult {
    let a: CreateWindowArgs = args(payload)?;
    if a._pad != [0; 4] {
        return Err(VarswapError::InvalidInstructionData.into());
    }
    let [payer, feed_config, window, system, ..] = accounts else {
        return Err(VarswapError::NotEnoughAccounts.into());
    };
    signer(payer)?;
    writable(payer)?;
    writable(window)?;
    system_program(system)?;
    let f = state::load_feed_config(feed_config, program_id)?;
    let spec = WindowSpec::try_new(
        pyth_lite::FeedId::new(f.feed),
        UnixTimestamp::new(a.start),
        UnixTimestamp::new(a.end),
        Seconds::new(a.interval),
        state::risk(&f),
    )
    .map_err(core)?;
    if a.interval < f.min_interval {
        return Err(VarswapError::IntervalBelowMinimum.into());
    }
    spec.check_creatable(now()?, f.max_buckets).map_err(core)?;

    let (start, end, interval) = (
        a.start.to_le_bytes(),
        a.end.to_le_bytes(),
        a.interval.to_le_bytes(),
    );
    let (expected, bump) = pda::find(&[WINDOW_SEED, &f.feed, &start, &end, &interval], program_id)?;
    if *window.address() != expected {
        return Err(VarswapError::InvalidPda.into());
    }
    let size = window_size(spec.buckets().map_err(core)?).map_err(core)?;
    let bump_seed = [bump];
    pda::create(
        window,
        size,
        program_id,
        payer,
        &seeds([WINDOW_SEED, &f.feed, &start, &end, &interval, &bump_seed]),
    )?;
    let mut data = window.try_borrow_mut()?;
    Window::init(&mut data, spec, bump).map_err(core)?;
    Ok(())
}

/// `sample`: anyone posts the canonical print of one or more buckets
/// (`docs/PLAN.md` 3.3, 4.1). The Pyth account must be owned by the receiver program the
/// window copied, `Full`, for the window's feed, positive, within the window's confidence
/// bound; staleness is not checked (canonical prints may be posted late, until the backfill
/// deadline). Return data: [`SampleResult`].
///
/// Accounts: window (writable), price update.
///
/// # Errors
///
/// A forged window, any `pyth-lite` check (codes 120 and up), any sampling rule of
/// `varswap_core::Window::sample` (140 and up).
#[inline(never)]
pub fn sample(program_id: &Address, accounts: &mut [AccountView], payload: &[u8]) -> ProgramResult {
    no_args(payload)?;
    let [window, price_update, ..] = accounts else {
        return Err(VarswapError::NotEnoughAccounts.into());
    };
    writable(window)?;
    owned(window, program_id)?;
    let now = now()?;
    let mut data = window.try_borrow_mut()?;
    let mut w = Window::load(&mut data).map_err(core)?;
    let spec = *w.spec();

    let raw = price_update.try_borrow()?;
    let update = PriceUpdate::load(
        price_update.owner().as_array(),
        &raw,
        &spec.receiver_program(),
    )
    .map_err(pyth)?;
    let req = PriceRequirements::new(spec.feed(), None, spec.max_conf().get());
    let price = update.verify(&req, now).map_err(pyth)?;
    let print = Print::try_from(&price).map_err(core)?;
    let before = w.sum_sq();
    let run = w.sample(&print, now, &Kernel::default()).map_err(core)?;

    let result = SampleResult {
        price: print.stored().price(),
        publish_time: print.publish_time().get(),
        first: run.first(),
        last: run.last(),
    };
    #[cfg(feature = "events")]
    {
        let added = w
            .sum_sq()
            .raw()
            .checked_sub(before.raw())
            .ok_or(VarswapError::InvalidAccountData)?;
        log_sample(&result, print.stored().exponent(), added);
    }
    #[cfg(not(feature = "events"))]
    let _ = before;
    set_return_data(bytemuck::bytes_of(&result));
    Ok(())
}

/// The sample event (`docs/PLAN.md` 3.1): enough to recompute `sum_sq` off-chain.
/// `varswap:sample first=<k> last=<k> publish_time=<s> price=<mantissa> expo=<e>
/// added=<raw sum_sq increment>`.
#[cfg(feature = "events")]
#[inline(never)]
fn log_sample(r: &SampleResult, exponent: i8, added: u128) {
    let mut l = pinocchio_log::logger::Logger::<160>::default();
    l.append("varswap:sample first=")
        .append(r.first)
        .append(" last=")
        .append(r.last)
        .append(" publish_time=")
        .append(r.publish_time)
        .append(" price=")
        .append(r.price)
        .append(" expo=")
        .append(i32::from(exponent))
        .append(" added=")
        .append(added);
    l.log();
}

/// `finalize`: anyone ends a window: `Finalized` once complete, `Degraded` (swaps unwind)
/// only after `end + backfill_deadline`.
///
/// Accounts: window (writable).
///
/// # Errors
///
/// A forged window, `WindowFinal` or `FinalizeTooEarly` (codes 140 and up).
#[inline(never)]
pub fn finalize(
    program_id: &Address,
    accounts: &mut [AccountView],
    payload: &[u8],
) -> ProgramResult {
    no_args(payload)?;
    let [window, ..] = accounts else {
        return Err(VarswapError::NotEnoughAccounts.into());
    };
    writable(window)?;
    owned(window, program_id)?;
    let now = now()?;
    let mut data = window.try_borrow_mut()?;
    let mut w = Window::load(&mut data).map_err(core)?;
    w.finalize(now, &Kernel::default()).map_err(core)?;
    Ok(())
}
