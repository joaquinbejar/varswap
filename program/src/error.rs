//! Program error codes (`ProgramError::Custom`), from 100 (matchbook convention).
//!
//! | Range | Source |
//! | --- | --- |
//! | 100..=139 | [`VarswapError`] (account and instruction validation) |
//! | 140..=159 | `140 +` the [`PythError`] discriminant |
//! | 160..=219 | `160 +` [`CoreError::code`] |
//! | 220..=255 | `220 +` the payload of [`CoreError::Kernel`] |

use pinocchio::error::ProgramError;
use pyth_lite::PythError;
use varswap_core::CoreError;

/// First code of the `PythError` range.
pub const PYTH_BASE: u32 = 140;
/// First code of the `CoreError` range.
pub const CORE_BASE: u32 = 160;
/// First code of the kernel error range.
pub const KERNEL_BASE: u32 = 220;

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
    /// `settle_swap` on a window that is neither `Finalized` nor `Degraded`.
    WindowNotFinal = 113,
    /// `open_swap` once the window's sampling started (`now >= start`) or it is final.
    WindowStarted = 114,
    /// `settle_swap` on a swap already settled.
    SwapAlreadySettled = 115,
    /// A claim on a swap not settled yet.
    SwapNotSettled = 116,
    /// That side already claimed.
    AlreadyClaimed = 117,
    /// `close_swap` before both sides claimed.
    NotClaimed = 118,
    /// The mint is not the configured collateral mint, or not the swap's.
    InvalidMint = 119,
    /// The token program is not SPL Token, or not the swap's.
    InvalidTokenProgram = 120,
    /// A token account of the wrong mint or owner.
    InvalidTokenAccount = 121,
    /// The vault is not the swap's `["vault", swap]` PDA.
    InvalidVault = 122,
    /// The window is not the swap's.
    WrongWindow = 123,
    /// The rent receiver is not the payer recorded in the swap.
    WrongReceiver = 124,
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
