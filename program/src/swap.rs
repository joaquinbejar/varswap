//! Swap instructions: `open_swap`, `settle_swap`, `claim_long` / `claim_short`, `close_swap`
//! (`docs/PLAN.md` 3.2, 3.3, 4.4). Each validates every account before use and writes the swap
//! before any token CPI.

use pinocchio::cpi::{Seed, Signer};
use pinocchio::sysvars::clock::Clock;
use pinocchio::sysvars::Sysvar;
use pinocchio::{AccountView, Address, ProgramResult};
use pinocchio_token::instructions::{CloseAccount, InitializeAccount3, TransferChecked};
use pinocchio_token::state::{Account as TokenAccount, Mint};
use varswap_core::{LnKernel, Settlement, SwapTerms, Window, WindowState};

use crate::error::{core, VarswapError};
use crate::layout::{OpenSwapArgs, SwapData};
use crate::pda::{self, SWAP_SEED, VAULT_SEED};
use crate::state::{
    self, hdr, owned, seeds, signer, system_program, writable, SwapState, SWAP_LEN, SWAP_TAG,
    TOKEN_ACCOUNT_LEN,
};

/// Which side a claim pays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// The long party.
    Long,
    /// The short party.
    Short,
}

fn token_program(a: &AccountView) -> ProgramResult {
    if *a.address() == pinocchio_token::ID {
        Ok(())
    } else {
        Err(VarswapError::InvalidTokenProgram.into())
    }
}

/// A token account of `mint` owned by `owner`.
fn token_account_of(a: &AccountView, mint: &Address, owner: &Address) -> ProgramResult {
    let t = TokenAccount::from_account_view(a).map_err(|_| VarswapError::InvalidTokenAccount)?;
    if t.mint() != mint || t.owner() != owner {
        return Err(VarswapError::InvalidTokenAccount.into());
    }
    Ok(())
}

fn decimals(mint: &AccountView) -> Result<u8, VarswapError> {
    Mint::from_account_view(mint)
        .map(|m| m.decimals())
        .map_err(|_| VarswapError::InvalidMint)
}

fn u128_of(b: [u8; 16]) -> u128 {
    u128::from_le_bytes(b)
}

/// `open_swap`: long and short both sign; the window has not started sampling; terms pass
/// [`SwapTerms::try_new`] (the error-bound gate included); collateral recomputed on-chain and
/// moved into a fresh `["vault", swap]` token account owned by the swap PDA.
///
/// Accounts: long (signer), short (signer), payer (signer, writable), config, window, swap
/// (writable), vault (writable), long token account (writable), short token account
/// (writable), mint, token program, system program.
///
/// # Errors
///
/// Missing signatures, wrong programs, a forged config or window, a started or final window,
/// invalid terms (codes 160 and up), wrong mint or token accounts, wrong PDAs, an existing
/// swap, or a token CPI failure (insufficient funds, frozen account).
#[inline(never)]
pub fn open_swap(
    program_id: &Address,
    accounts: &mut [AccountView],
    payload: &[u8],
) -> ProgramResult {
    let a: OpenSwapArgs = bytemuck::try_pod_read_unaligned(payload)
        .map_err(|_| VarswapError::InvalidInstructionData)?;
    let [long, short, payer, config, window, swap, vault, long_src, short_src, mint, token, system, ..] =
        accounts
    else {
        return Err(VarswapError::NotEnoughAccounts.into());
    };
    signer(long)?;
    signer(short)?;
    signer(payer)?;
    writable(payer)?;
    writable(swap)?;
    writable(vault)?;
    writable(long_src)?;
    writable(short_src)?;
    system_program(system)?;
    token_program(token)?;
    let c = state::load_config(config, program_id)?;
    if mint.address().to_bytes() != c.collateral_mint {
        return Err(VarswapError::InvalidMint.into());
    }
    let decimals = decimals(mint)?;

    owned(window, program_id)?;
    let info = Window::inspect(&window.try_borrow()?).map_err(core)?;
    let now = Clock::get()?.unix_timestamp;
    if info.state.is_final() || now >= info.spec.start().get() {
        return Err(VarswapError::WindowStarted.into());
    }
    let terms = SwapTerms::try_new(
        a.notional,
        u128_of(a.strike_var),
        u128_of(a.cap_var),
        info.spec.interval(),
    )
    .map_err(core)?;
    token_account_of(long_src, mint.address(), long.address())?;
    token_account_of(short_src, mint.address(), short.address())?;

    let index = a.swap_index.to_le_bytes();
    let (swap_addr, swap_bump) = pda::find(
        &[
            SWAP_SEED,
            window.address().as_ref(),
            long.address().as_ref(),
            short.address().as_ref(),
            &index,
        ],
        program_id,
    )?;
    if *swap.address() != swap_addr {
        return Err(VarswapError::InvalidPda.into());
    }
    let (vault_addr, vault_bump) = pda::find(&[VAULT_SEED, swap_addr.as_ref()], program_id)?;
    if *vault.address() != vault_addr {
        return Err(VarswapError::InvalidVault.into());
    }

    let swap_bump_seed = [swap_bump];
    pda::create(
        swap,
        SWAP_LEN,
        program_id,
        payer,
        &seeds([
            SWAP_SEED,
            window.address().as_ref(),
            long.address().as_ref(),
            short.address().as_ref(),
            &index,
            &swap_bump_seed,
        ]),
    )?;
    state::write(
        swap,
        &SwapData {
            hdr: hdr(SWAP_TAG, swap_bump),
            window: window.address().to_bytes(),
            long: long.address().to_bytes(),
            short: short.address().to_bytes(),
            mint: mint.address().to_bytes(),
            token_program: token.address().to_bytes(),
            payer: payer.address().to_bytes(),
            notional: terms.notional(),
            swap_index: a.swap_index,
            long_collateral: terms.long_collateral(),
            short_collateral: terms.short_collateral(),
            long_payout: 0,
            short_payout: 0,
            strike_var: terms.strike_var().to_le_bytes(),
            cap_var: terms.cap_var().to_le_bytes(),
            state: SwapState::Open as u8,
            long_claimed: 0,
            short_claimed: 0,
            vault_bump,
            _pad: [0; 4],
        },
    )?;

    let vault_bump_seed = [vault_bump];
    pda::create(
        vault,
        TOKEN_ACCOUNT_LEN,
        &pinocchio_token::ID,
        payer,
        &seeds([VAULT_SEED, swap_addr.as_ref(), &vault_bump_seed]),
    )?;
    InitializeAccount3::new(vault, mint, &swap_addr).invoke()?;
    TransferChecked::<&AccountView>::new(
        long_src,
        mint,
        vault,
        long,
        terms.long_collateral(),
        decimals,
    )
    .invoke()?;
    TransferChecked::<&AccountView>::new(
        short_src,
        mint,
        vault,
        short,
        terms.short_collateral(),
        decimals,
    )
    .invoke()
}

/// `settle_swap`: anyone, once the window is final. `Finalized`: payouts on the conservative
/// realized variance `max(0, V − ε)` ([`LnKernel::conservative_realized_var`], before the cap
/// and the floor); `Degraded`: each side's collateral back. Moves no tokens.
///
/// Accounts: window, swap (writable).
///
/// # Errors
///
/// A forged window or swap, another window, a window not final, a swap already settled.
#[inline(never)]
pub fn settle_swap(
    program_id: &Address,
    accounts: &mut [AccountView],
    payload: &[u8],
) -> ProgramResult {
    if !payload.is_empty() {
        return Err(VarswapError::InvalidInstructionData.into());
    }
    let [window, swap, ..] = accounts else {
        return Err(VarswapError::NotEnoughAccounts.into());
    };
    writable(swap)?;
    let (mut s, st) = state::load_swap(swap, program_id)?;
    if st != SwapState::Open {
        return Err(VarswapError::SwapAlreadySettled.into());
    }
    if window.address().to_bytes() != s.window {
        return Err(VarswapError::WrongWindow.into());
    }
    owned(window, program_id)?;
    let info = Window::inspect(&window.try_borrow()?).map_err(core)?;
    let terms = SwapTerms::try_new(
        s.notional,
        u128_of(s.strike_var),
        u128_of(s.cap_var),
        info.spec.interval(),
    )
    .map_err(core)?;
    let settlement = match (info.state, info.realized_var) {
        (WindowState::Finalized, Some(v)) => Settlement::Realized(
            LnKernel::conservative_realized_var(v, info.spec.interval(), terms.cap_var(), 0)
                .map_err(core)?,
        ),
        (WindowState::Degraded, _) => Settlement::Unwind,
        _ => return Err(VarswapError::WindowNotFinal.into()),
    };
    let (long, short) = terms.payouts(settlement).map_err(core)?;
    s.long_payout = long;
    s.short_payout = short;
    s.state = SwapState::Settled as u8;
    state::write(swap, &s)
}

/// `claim_long` / `claim_short`: anyone pays one side its payout, into a token account of the
/// swap's mint owned by that side. One side's frozen account never blocks the other side.
///
/// Accounts: swap (writable), vault (writable), destination token account (writable), mint,
/// token program.
///
/// # Errors
///
/// A forged swap, a swap not settled, a side already paid, a wrong vault, mint, token program
/// or destination, or the transfer failing (a frozen destination).
#[inline(never)]
pub fn claim(
    program_id: &Address,
    accounts: &mut [AccountView],
    payload: &[u8],
    side: Side,
) -> ProgramResult {
    if !payload.is_empty() {
        return Err(VarswapError::InvalidInstructionData.into());
    }
    let [swap, vault, dest, mint, token, ..] = accounts else {
        return Err(VarswapError::NotEnoughAccounts.into());
    };
    writable(swap)?;
    writable(vault)?;
    writable(dest)?;
    let (mut s, st) = state::load_swap(swap, program_id)?;
    if st != SwapState::Settled {
        return Err(VarswapError::SwapNotSettled.into());
    }
    if token.address().to_bytes() != s.token_program {
        return Err(VarswapError::InvalidTokenProgram.into());
    }
    if mint.address().to_bytes() != s.mint {
        return Err(VarswapError::InvalidMint.into());
    }
    pda::verify(
        &[VAULT_SEED, swap.address().as_ref(), &[s.vault_bump]],
        program_id,
        vault.address(),
    )
    .map_err(|_| VarswapError::InvalidVault)?;
    let (owner, amount) = match side {
        Side::Long => {
            if s.long_claimed != 0 {
                return Err(VarswapError::AlreadyClaimed.into());
            }
            s.long_claimed = 1;
            (s.long, s.long_payout)
        }
        Side::Short => {
            if s.short_claimed != 0 {
                return Err(VarswapError::AlreadyClaimed.into());
            }
            s.short_claimed = 1;
            (s.short, s.short_payout)
        }
    };
    token_account_of(dest, mint.address(), &Address::new_from_array(owner))?;
    state::write(swap, &s)?;
    if amount == 0 {
        return Ok(());
    }
    let decimals = decimals(mint)?;
    let index = s.swap_index.to_le_bytes();
    let bump = [s.hdr.bump];
    let signer_seeds = [
        Seed::from(SWAP_SEED),
        Seed::from(&s.window),
        Seed::from(&s.long),
        Seed::from(&s.short),
        Seed::from(&index),
        Seed::from(&bump),
    ];
    TransferChecked::<&AccountView>::new(vault, mint, dest, swap, amount, decimals)
        .invoke_signed(&[Signer::from(&signer_seeds)])
}

/// `close_swap`: anyone, after both sides claimed: closes the empty vault and the swap, and
/// returns both rents to the payer recorded at opening.
///
/// Accounts: swap (writable), vault (writable), rent receiver (writable), token program.
///
/// # Errors
///
/// A forged swap, a side not yet paid, a wrong vault, receiver or token program, or a vault
/// that is not empty.
#[inline(never)]
pub fn close_swap(
    program_id: &Address,
    accounts: &mut [AccountView],
    payload: &[u8],
) -> ProgramResult {
    if !payload.is_empty() {
        return Err(VarswapError::InvalidInstructionData.into());
    }
    let [swap, vault, receiver, token, ..] = accounts else {
        return Err(VarswapError::NotEnoughAccounts.into());
    };
    writable(swap)?;
    writable(vault)?;
    writable(receiver)?;
    let (s, st) = state::load_swap(swap, program_id)?;
    if st != SwapState::Settled || s.long_claimed == 0 || s.short_claimed == 0 {
        return Err(VarswapError::NotClaimed.into());
    }
    if receiver.address().to_bytes() != s.payer {
        return Err(VarswapError::WrongReceiver.into());
    }
    if token.address().to_bytes() != s.token_program {
        return Err(VarswapError::InvalidTokenProgram.into());
    }
    pda::verify(
        &[VAULT_SEED, swap.address().as_ref(), &[s.vault_bump]],
        program_id,
        vault.address(),
    )
    .map_err(|_| VarswapError::InvalidVault)?;
    let index = s.swap_index.to_le_bytes();
    let bump = [s.hdr.bump];
    let signer_seeds = [
        Seed::from(SWAP_SEED),
        Seed::from(&s.window),
        Seed::from(&s.long),
        Seed::from(&s.short),
        Seed::from(&index),
        Seed::from(&bump),
    ];
    CloseAccount::<&AccountView>::new(vault, receiver, swap)
        .invoke_signed(&[Signer::from(&signer_seeds)])?;
    let rent = swap.lamports();
    let total = receiver
        .lamports()
        .checked_add(rent)
        .ok_or(VarswapError::InvalidAccountData)?;
    receiver.set_lamports(total);
    swap.set_lamports(0);
    swap.close()
}
