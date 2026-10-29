//! Program error codes (`ProgramError::Custom`), from 100 (matchbook convention).
//!
//! | Range | Source |
//! | --- | --- |
//! | 100..=119 | [`VarswapError`] (account and instruction validation) |
//! | 120..=139 | `120 +` the [`PythError`] discriminant |
//! | 140..=199 | `140 +` [`CoreError::code`] |
//! | 200..=255 | `200 +` the payload of [`CoreError::Kernel`] |

use pinocchio::error::ProgramError;
use pyth_lite::PythError;
use varswap_core::CoreError;

/// First code of the `PythError` range.
pub const PYTH_BASE: u32 = 120;
/// First code of the `CoreError` range.
pub const CORE_BASE: u32 = 140;
/// First code of the kernel error range.
pub const KERNEL_BASE: u32 = 200;

/// Account and instruction validation errors of the program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum VarswapError {
    /// Empty instruction data or an unassigned tag.
    InvalidInstruction = 100,
    /// The payload has the wrong length or invalid padding.
    InvalidInstructionData = 101,
    /// Fewer accounts than the instruction needs.
    NotEnoughAccounts = 102,
    /// A required signer did not sign.
    MissingSignature = 103,
    /// An account that is written is not writable.
    NotWritable = 104,
    /// An account is owned by the wrong program.
    InvalidAccountOwner = 105,
    /// An account is not at the expected PDA.
    InvalidPda = 106,
    /// An account's bytes are not a valid account of the expected kind.
    InvalidAccountData = 107,
    /// The system program account is not the system program.
    InvalidSystemProgram = 108,
    /// The signer is not the config admin, or not the program's upgrade authority.
    NotAuthority = 109,
    /// The account to create already exists.
    AlreadyInitialized = 110,
    /// `set_feed_config` parameters out of range (`min_interval`, `max_buckets`, receiver).
    InvalidFeedConfig = 111,
    /// `create_window` interval below the feed's `min_interval`.
    IntervalBelowMinimum = 112,
}

impl From<VarswapError> for ProgramError {
    #[cold]
    #[inline(never)]
    fn from(e: VarswapError) -> Self {
        ProgramError::Custom(e as u32)
    }
}

/// `ProgramError::Custom(120 + discriminant)`.
#[cold]
#[inline(never)]
#[must_use]
pub fn pyth(e: PythError) -> ProgramError {
    ProgramError::Custom(PYTH_BASE.saturating_add(u32::from(e as u8)))
}

/// `ProgramError::Custom(140 + code)`, or `200 + payload` for a kernel error.
#[cold]
#[inline(never)]
#[must_use]
pub fn core(e: CoreError) -> ProgramError {
    // Error codes, not financial values: saturating cannot change an amount.
    match e {
        CoreError::Kernel(c) => ProgramError::Custom(KERNEL_BASE.saturating_add(u32::from(c))),
        e => ProgramError::Custom(CORE_BASE.saturating_add(u32::from(e.code()))),
    }
}
