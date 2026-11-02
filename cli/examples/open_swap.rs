//! Opens one swap, for the I8 devnet run (`docs/PLAN-improvements.md`): `varswap-admin` has
//! no `open-swap` (both parties sign it, which a single-keypair CLI does not model), so this
//! example builds it with `varswap-sdk` and signs with the long, the short and the payer.
//!
//! ```sh
//! SOLANA_URL=https://api.devnet.solana.com VARSWAP_PROGRAM_ID=<program id> \
//!   cargo run -p varswap-cli --example open_swap -- \
//!   <window> <long keypair> <short keypair> <payer keypair> <notional atoms> <strike ppm> <cap ppm>
//! ```
//!
//! Strike and cap are annualized variances in millionths (10000 = 0.01). Collateral comes
//! from each side's associated token account of the config's collateral mint.

use std::process::ExitCode;

use solana_signer::Signer;
use solana_transaction::Transaction;
use varswap_cli::rpc::Cluster;
use varswap_cli::{parse_address, read_config, read_keypair, CliError, Rpc};
use varswap_sdk::ix::{self, OpenSwap};
use varswap_sdk::{pda, Address};

/// Q64.64 one.
const ONE: u128 = 1 << 64;

fn ppm(arg: &str) -> Result<u128, CliError> {
    let v: u128 = arg
        .parse()
        .map_err(|_| CliError::Usage(format!("{arg} is not a whole number of millionths")))?;
    // Rounded down, explicitly: millionths of a variance unit to Q64.64 raw.
    v.checked_mul(ONE)
        .and_then(|x| x.checked_div_euclid(1_000_000))
        .ok_or_else(|| CliError::Usage(format!("{arg} is too large")))
}

fn run() -> Result<String, CliError> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [window, long, short, payer, notional, strike, cap] = args.as_slice() else {
        return Err(CliError::Usage(
            "usage: open_swap <window> <long keypair> <short keypair> <payer keypair> <notional atoms> <strike ppm> <cap ppm>".to_owned(),
        ));
    };
    let url = std::env::var("SOLANA_URL").map_err(|_| CliError::Usage("SOLANA_URL".to_owned()))?;
    let program_id = parse_address(
        &std::env::var("VARSWAP_PROGRAM_ID")
            .map_err(|_| CliError::Usage("VARSWAP_PROGRAM_ID".to_owned()))?,
    )?;
    let window = parse_address(window)?;
    let (long, short, payer) = (
        read_keypair(long)?,
        read_keypair(short)?,
        read_keypair(payer)?,
    );
    let mut rpc = Cluster::new(&url);
    let mint = Address::new_from_array(read_config(&mut rpc, &program_id)?.collateral_mint);
    let o = OpenSwap {
        window,
        long: long.pubkey(),
        short: short.pubkey(),
        payer: payer.pubkey(),
        long_src: pda::ata(&long.pubkey(), &mint),
        short_src: pda::ata(&short.pubkey(), &mint),
        mint,
        notional: notional
            .parse()
            .map_err(|_| CliError::Usage(format!("{notional} is not a number of atoms")))?,
        strike_var: ppm(strike)?,
        cap_var: ppm(cap)?,
        index: 0,
    };
    let swap = pda::swap(&program_id, &window, &o.long, &o.short, o.index).0;
    let instruction = ix::open_swap(&program_id, &o);
    let blockhash = rpc.latest_blockhash()?;
    let mut tx = Transaction::new_with_payer(&[instruction], Some(&payer.pubkey()));
    tx.try_sign(&[&payer, &long, &short], blockhash)
        .map_err(|_| CliError::Sign("open_swap needs the long, the short and the payer"))?;
    let signature = tx
        .signatures
        .first()
        .map(ToString::to_string)
        .unwrap_or_default();
    rpc.send(&tx)?;
    Ok(format!("swap {swap} opened: {signature}"))
}

fn main() -> ExitCode {
    match run() {
        Ok(line) => {
            println!("{line}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
