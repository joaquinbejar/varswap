//! varswap on-chain program: realized-variance windows over canonical Pyth prints
//! (`docs/PLAN.md` 3).
//!
//! Pinocchio, `no_std`, no allocator. Instruction data is a one-byte [`Instruction`] tag
//! followed by a fixed-size little-endian payload ([`layout`]). The accumulator rules live in
//! `varswap-core`; this crate validates accounts, reads the clock and the Pyth account, and
//! maps errors to codes ([`error`]).
//!
//! Milestone 3a: the squared log return is `varswap_core::IntegerKernel`, an exact integer
//! stand-in, until milestone 1b plugs in optfix `ln`. It exists only with the
//! `insecure-test-kernel` feature; without it the crate refuses to compile, so no deployable
//! ELF can be built (`make sbf-control` checks that the default `cargo build-sbf` fails).

#![no_std]

pub mod error;
pub mod layout;
pub mod loader;
pub mod pda;
pub mod processor;
pub mod state;

use pinocchio::{AccountView, Address, ProgramResult};

use crate::error::VarswapError;

// The macros expand to the `extern "C"` entrypoint and the target's allocator and panic hooks;
// that generated code is the only `unsafe` in this crate. `no_allocator!` also defines
// `allocate_unchecked`, whose offset helper asserts its bounds; nothing here calls it.
#[allow(unsafe_code, clippy::disallowed_macros)]
mod entry {
    pinocchio::program_entrypoint!(super::process_instruction);
    pinocchio::no_allocator!();
    pinocchio::nostd_panic_handler!();
}

#[cfg(not(feature = "insecure-test-kernel"))]
compile_error!(
    "varswap-program has no production kernel before milestone 1b (optfix ln); \
     `--features insecure-test-kernel` builds the integer stand-in for tests only"
);

/// Kernel the program runs (milestone 3a stand-in, see the crate docs).
#[cfg(feature = "insecure-test-kernel")]
pub type Kernel = varswap_core::IntegerKernel;

/// Instruction tags (the first byte of the instruction data).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Instruction {
    /// `init_config`: admin (the program's upgrade authority), payer, config, program data,
    /// system program. No payload.
    InitConfig = 0,
    /// `set_feed_config`: admin, payer, config, feed config, system program.
    /// Payload [`layout::SetFeedConfigArgs`].
    SetFeedConfig = 1,
    /// `create_window`: payer, feed config, window, system program.
    /// Payload [`layout::CreateWindowArgs`].
    CreateWindow = 2,
    /// `sample`: window, Pyth price update. No payload. Return data [`layout::SampleResult`].
    Sample = 3,
    /// `finalize`: window. No payload.
    Finalize = 4,
}

impl Instruction {
    /// Decodes a tag byte.
    ///
    /// # Errors
    ///
    /// [`VarswapError::InvalidInstruction`] for an unassigned tag.
    #[inline]
    pub fn from_tag(tag: u8) -> Result<Self, VarswapError> {
        match tag {
            0 => Ok(Self::InitConfig),
            1 => Ok(Self::SetFeedConfig),
            2 => Ok(Self::CreateWindow),
            3 => Ok(Self::Sample),
            4 => Ok(Self::Finalize),
            _ => Err(VarswapError::InvalidInstruction),
        }
    }
}

/// Program entrypoint body.
///
/// # Errors
///
/// Any error of the instruction handlers, as a `ProgramError::Custom` code ([`error`]) or a
/// runtime `ProgramError`.
pub fn process_instruction(
    program_id: &Address,
    accounts: &mut [AccountView],
    data: &[u8],
) -> ProgramResult {
    let (&tag, payload) = data.split_first().ok_or(VarswapError::InvalidInstruction)?;
    match Instruction::from_tag(tag)? {
        Instruction::InitConfig => processor::init_config(program_id, accounts, payload),
        Instruction::SetFeedConfig => processor::set_feed_config(program_id, accounts, payload),
        Instruction::CreateWindow => processor::create_window(program_id, accounts, payload),
        Instruction::Sample => processor::sample(program_id, accounts, payload),
        Instruction::Finalize => processor::finalize(program_id, accounts, payload),
    }
}
