//! Thin admin CLI for varswap (`varswap-admin`): `init_config`, `set_feed_config` and
//! `create_window` built by `varswap-sdk`, signed with a keypair file and sent over JSON-RPC,
//! plus decoded reads of the config, feed configs, windows and swaps.
//!
//! The commands are written against the [`Rpc`] trait: [`rpc::Cluster`] (`solana-rpc-client`)
//! is the network implementation, and the LiteSVM tests drive the same commands through an
//! in-process one.

pub mod error;
pub mod rpc;

use serde_json::{json, Value};
use solana_hash::Hash;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_transaction::Transaction;
use varswap_core::layout::{ConfigData, FeedConfigData, SwapData};
use varswap_core::{WindowInfo, WindowState};
use varswap_sdk::ix::FeedParams;
use varswap_sdk::pda::{self, WindowKey};
use varswap_sdk::{decode, ix, Address};

pub use error::CliError;

/// An account as the RPC returns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountInfo {
    /// The owning program.
    pub owner: Address,
    /// The account data.
    pub data: Vec<u8>,
}

/// What the commands need from a Solana node.
pub trait Rpc {
    /// A recent blockhash.
    ///
    /// # Errors
    ///
    /// [`CliError`] from the node or the transport.
    fn latest_blockhash(&mut self) -> Result<Hash, CliError>;

    /// The account at `address`, `None` if it does not exist.
    ///
    /// # Errors
    ///
    /// [`CliError`] from the node or the transport.
    fn account(&mut self, address: &Address) -> Result<Option<AccountInfo>, CliError>;

    /// Sends a signed transaction and waits until it is confirmed.
    ///
    /// # Errors
    ///
    /// [`CliError`] when it is rejected, fails or is not confirmed in time.
    fn send(&mut self, tx: &Transaction) -> Result<(), CliError>;
}

/// Reads a Solana CLI keypair file (a JSON array of 64 bytes). The error names the path, never
/// the content.
///
/// # Errors
///
/// [`CliError::Keypair`].
pub fn read_keypair(path: &str) -> Result<Keypair, CliError> {
    let mut file = std::fs::File::open(path).map_err(|_| CliError::Keypair {
        path: path.to_owned(),
        what: "cannot be opened",
    })?;
    solana_keypair::read_keypair(&mut file).map_err(|_| CliError::Keypair {
        path: path.to_owned(),
        what: "is not a JSON array of 64 bytes forming a valid keypair",
    })
}

/// Signs `instructions` with `signer` as fee payer and only signer, sends them and waits for
/// confirmation. Returns the transaction signature.
///
/// # Errors
///
/// [`CliError`] from signing, the node or the transport.
pub fn submit<R: Rpc>(
    rpc: &mut R,
    instructions: &[Instruction],
    signer: &Keypair,
) -> Result<String, CliError> {
    let blockhash = rpc.latest_blockhash()?;
    let mut tx = Transaction::new_with_payer(instructions, Some(&signer.pubkey()));
    tx.try_sign(&[signer], blockhash)
        .map_err(|_| CliError::Sign("the instructions need another signer"))?;
    let signature = tx
        .signatures
        .first()
        .map(ToString::to_string)
        .ok_or(CliError::Sign("no signature"))?;
    rpc.send(&tx)?;
    Ok(signature)
}

/// A human-readable dump of an instruction, for `--dry-run`.
#[must_use]
pub fn describe(instruction: &Instruction) -> String {
    let mut out = format!("program {}\naccounts\n", instruction.program_id);
    for (i, meta) in instruction.accounts.iter().enumerate() {
        let flags = match (meta.is_signer, meta.is_writable) {
            (true, true) => " signer, writable",
            (true, false) => " signer",
            (false, true) => " writable",
            (false, false) => "",
        };
        out.push_str(&format!("  {i:>2} {}{flags}\n", meta.pubkey));
    }
    let hex: String = instruction
        .data
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    out.push_str(&format!("data ({} bytes) {hex}\n", instruction.data.len()));
    out
}

/// Parses a base58 address.
///
/// # Errors
///
/// [`CliError::Usage`].
pub fn parse_address(s: &str) -> Result<Address, CliError> {
    s.parse::<Address>()
        .map_err(|_| CliError::Usage(format!("{s:?} is not a base58 address")))
}

/// Parses a 32-byte Pyth feed id, hex with or without `0x`.
///
/// # Errors
///
/// [`CliError::Usage`].
pub fn parse_feed_id(s: &str) -> Result<[u8; 32], CliError> {
    let hex = s.trim().trim_start_matches("0x");
    let bad = || CliError::Usage(format!("{s:?} is not a 32-byte hex feed id"));
    if hex.len() != 64 || !hex.is_ascii() {
        return Err(bad());
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        let pair = hex
            .get(
                i.checked_mul(2).ok_or_else(bad)?
                    ..i.checked_mul(2)
                        .and_then(|j| j.checked_add(2))
                        .ok_or_else(bad)?,
            )
            .ok_or_else(bad)?;
        *byte = u8::from_str_radix(pair, 16).map_err(|_| bad())?;
    }
    Ok(out)
}

/// Lower-case hex of a feed id.
#[must_use]
pub fn hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A variance in Q64.64 raw units as a decimal string (for display only).
#[must_use]
pub fn format_var(raw: u128) -> String {
    // integer part and 12 decimals, rounded down
    let int = raw >> 64;
    let frac = raw & u128::from(u64::MAX);
    let decimals = frac.checked_mul(1_000_000_000_000).map_or(0, |x| x >> 64);
    format!("{int}.{decimals:012}")
}

fn fetch<R: Rpc>(rpc: &mut R, address: &Address) -> Result<AccountInfo, CliError> {
    rpc.account(address)?
        .ok_or_else(|| CliError::AccountNotFound(address.to_string()))
}

fn decode_err(address: &Address, what: &'static str) -> CliError {
    CliError::Decode {
        address: address.to_string(),
        what,
    }
}

/// `init_config`: `signer` is the program's upgrade authority (becomes admin) and pays.
#[must_use]
pub fn init_config_ix(
    program_id: &Address,
    signer: &Address,
    collateral_mint: &Address,
) -> Instruction {
    ix::init_config(program_id, signer, signer, collateral_mint)
}

/// `set_feed_config`: `signer` is the admin and pays a new feed config's rent.
#[must_use]
pub fn set_feed_config_ix(program_id: &Address, signer: &Address, p: &FeedParams) -> Instruction {
    ix::set_feed_config(program_id, signer, signer, p)
}

/// `create_window`: `signer` pays the window's rent. Checks the grid before signing.
///
/// # Errors
///
/// [`CliError::Usage`] if `end` is not after `start` or not on the grid.
pub fn create_window_ix(
    program_id: &Address,
    signer: &Address,
    k: &WindowKey,
) -> Result<Instruction, CliError> {
    let len = k
        .end
        .checked_sub(k.start)
        .filter(|l| *l > 0)
        .ok_or_else(|| CliError::Usage("--end must be after --start".into()))?;
    if k.interval == 0 || len.checked_rem(i64::from(k.interval)) != Some(0) {
        return Err(CliError::Usage(
            "--end − --start must be a positive multiple of --interval".into(),
        ));
    }
    Ok(ix::create_window(program_id, signer, k))
}

/// The live config.
///
/// # Errors
///
/// [`CliError::AccountNotFound`] before `init_config`, [`CliError::Decode`] for a foreign
/// account, or a node error.
pub fn read_config<R: Rpc>(rpc: &mut R, program_id: &Address) -> Result<ConfigData, CliError> {
    let a = pda::config(program_id).0;
    let info = fetch(rpc, &a)?;
    decode::config(program_id, &info.owner, &info.data).map_err(|_| decode_err(&a, "config"))
}

/// A feed's config.
///
/// # Errors
///
/// As [`read_config`].
pub fn read_feed_config<R: Rpc>(
    rpc: &mut R,
    program_id: &Address,
    feed: &[u8; 32],
) -> Result<FeedConfigData, CliError> {
    let a = pda::feed_config(program_id, feed).0;
    let info = fetch(rpc, &a)?;
    decode::feed_config(program_id, &info.owner, &info.data)
        .map_err(|_| decode_err(&a, "feed config"))
}

/// A window's spec, state, realized variance and filled buckets.
///
/// # Errors
///
/// As [`read_config`].
pub fn read_window<R: Rpc>(
    rpc: &mut R,
    program_id: &Address,
    window: &Address,
) -> Result<(WindowInfo, Vec<bool>), CliError> {
    let info = fetch(rpc, window)?;
    let w = decode::window(program_id, &info.owner, &info.data)
        .map_err(|_| decode_err(window, "window"))?;
    let filled = decode::filled_buckets(program_id, &info.owner, &info.data)
        .map_err(|_| decode_err(window, "window"))?;
    Ok((w, filled))
}

/// A swap.
///
/// # Errors
///
/// As [`read_config`].
pub fn read_swap<R: Rpc>(
    rpc: &mut R,
    program_id: &Address,
    swap: &Address,
) -> Result<SwapData, CliError> {
    let info = fetch(rpc, swap)?;
    decode::swap(program_id, &info.owner, &info.data).map_err(|_| decode_err(swap, "swap"))
}

/// The config as JSON.
#[must_use]
pub fn config_json(program_id: &Address, c: &ConfigData) -> Value {
    json!({
        "address": pda::config(program_id).0.to_string(),
        "admin": Address::new_from_array(c.admin).to_string(),
        "collateral_mint": Address::new_from_array(c.collateral_mint).to_string(),
    })
}

/// A feed config as JSON.
#[must_use]
pub fn feed_config_json(program_id: &Address, f: &FeedConfigData) -> Value {
    json!({
        "address": pda::feed_config(program_id, &f.feed).0.to_string(),
        "feed": hex(&f.feed),
        "receiver_program": Address::new_from_array(f.receiver_program).to_string(),
        "min_interval_s": f.min_interval,
        "tolerance_s": f.tolerance,
        "backfill_deadline_s": f.backfill_deadline,
        "max_buckets": f.max_buckets,
        "max_conf_bps": f.max_conf,
        "exponent": f.exponent,
    })
}

fn state_name(s: WindowState) -> &'static str {
    match s {
        WindowState::Pending => "pending",
        WindowState::Sampling => "sampling",
        WindowState::Finalized => "finalized",
        WindowState::Degraded => "degraded",
    }
}

/// A window as JSON.
#[must_use]
pub fn window_json(address: &Address, w: &WindowInfo, filled: &[bool]) -> Value {
    let s = &w.spec;
    let missing: Vec<usize> = filled
        .iter()
        .enumerate()
        .filter(|(_, f)| !**f)
        .map(|(k, _)| k)
        .collect();
    json!({
        "address": address.to_string(),
        "feed": hex(&s.feed().to_bytes()),
        "start": s.start().get(),
        "end": s.end().get(),
        "interval_s": s.interval().get(),
        "tolerance_s": s.tolerance().get(),
        "backfill_deadline_s": s.backfill_deadline().get(),
        "max_conf_bps": s.max_conf().get(),
        "exponent": s.exponent(),
        "receiver_program": Address::new_from_array(s.receiver_program()).to_string(),
        "state": state_name(w.state),
        "buckets": filled.len(),
        "filled": filled.iter().filter(|f| **f).count(),
        "missing": missing,
        "realized_var": w.realized_var.map(format_var),
    })
}

/// A swap as JSON.
#[must_use]
pub fn swap_json(address: &Address, s: &SwapData) -> Value {
    json!({
        "address": address.to_string(),
        "window": Address::new_from_array(s.window).to_string(),
        "long": Address::new_from_array(s.long).to_string(),
        "short": Address::new_from_array(s.short).to_string(),
        "mint": Address::new_from_array(s.mint).to_string(),
        "payer": Address::new_from_array(s.payer).to_string(),
        "notional": s.notional,
        "swap_index": s.swap_index,
        "strike_var": format_var(u128::from_le_bytes(s.strike_var)),
        "cap_var": format_var(u128::from_le_bytes(s.cap_var)),
        "long_collateral": s.long_collateral,
        "short_collateral": s.short_collateral,
        "state": if s.state == 1 { "settled" } else { "open" },
        "long_payout": s.long_payout,
        "short_payout": s.short_payout,
        "long_claimed": s.long_claimed == 1,
        "short_claimed": s.short_claimed == 1,
    })
}
