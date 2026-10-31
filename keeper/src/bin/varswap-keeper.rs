//! `varswap-keeper`: samples varswap windows on a cluster until they are final.
//!
//! Each tick posts every published canonical print a window is missing (Hermes, then Wormhole
//! and the Pyth receiver, then `sample` in the same transaction) and finalizes the window once
//! it is complete or past its backfill deadline. With `--watch SECONDS` it repeats until every
//! window is final, scans the payer's encoded VAAs at start and every 12 rounds (one seen for
//! 300 blocks or more is a leftover, closed before the next post), and backs off at least 61 s after a Hermes 429 (the key's limit, about 10
//! requests a minute, is shared). Every transaction signature is printed, one per line.
//!
//! Configuration comes from the environment: `SOLANA_URL`, `VARSWAP_PROGRAM_ID`,
//! `VARSWAP_KEYPAIR` (a keypair file path), and `VARSWAP_HERMES_API_KEY` (never printed),
//! with the optional `VARSWAP_HERMES_URL` and `VARSWAP_HERMES_AUTH`. The payer must be
//! exclusive to this process: it takes `pyth_post::PayerLock` at start (a lock file in
//! `$PYTH_POST_LOCK_DIR`, default `~/.local/state/pyth-post`) and refuses to run if another
//! process holds it.

use std::process::ExitCode;
use std::time::Duration;

use clap::Parser;
use solana_keypair::Keypair;
use solana_signer::Signer;
use varswap_keeper::{
    HermesConfig, HermesSource, Keeper, KeeperError, ReceiverChain, RpcTransport, UreqHttp,
};
use varswap_sdk::Address;

/// Samples varswap windows until they are final.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// JSON-RPC URL. Never printed, so a provider key in it stays private.
    #[arg(long, env = "SOLANA_URL", hide_env_values = true)]
    url: String,
    /// The varswap program id.
    #[arg(long, env = "VARSWAP_PROGRAM_ID")]
    program_id: String,
    /// Keypair file of the payer (fees only: every account a post opens is closed).
    #[arg(long, env = "VARSWAP_KEYPAIR")]
    keypair: String,
    /// Repeat every SECONDS until every window is final (default: one tick).
    #[arg(long, value_name = "SECONDS")]
    watch: Option<u64>,
    /// Window addresses.
    #[arg(required = true)]
    windows: Vec<String>,
}

fn read_keypair(path: &str) -> Result<Keypair, String> {
    let mut file =
        std::fs::File::open(path).map_err(|_| format!("keypair {path} cannot be opened"))?;
    solana_keypair::read_keypair(&mut file)
        .map_err(|_| format!("keypair {path} is not a JSON array of 64 bytes"))
}

fn address(s: &str) -> Result<Address, String> {
    s.parse()
        .map_err(|_| format!("{s} is not a base58 address"))
}

fn run(args: &Args) -> Result<(), String> {
    let program_id = address(&args.program_id)?;
    let windows = args
        .windows
        .iter()
        .map(|w| address(w))
        .collect::<Result<Vec<_>, _>>()?;
    let payer = read_keypair(&args.keypair)?;
    // One poster process per payer (pyth-post's contract): held until the process exits.
    let _lock = pyth_post::PayerLock::acquire(&payer.pubkey()).map_err(|e| e.to_string())?;
    // One source for the price queries and the posts: one cache, one rate limit.
    let source = HermesConfig::from_env()
        .map(|c| HermesSource::new(c, UreqHttp::new()))
        .map_err(|e: KeeperError| e.to_string())?;
    let mut chain = ReceiverChain::new(RpcTransport::new(&args.url), &source, payer, program_id);
    let keeper = Keeper::new(program_id);
    let mut printed = 0;
    let mut ticks: u64 = 0;
    loop {
        // A leftover encoded VAA (a transaction that landed after its post gave up on it) is
        // found by a scan of the payer's encoded VAAs: at start, then every 12 rounds.
        if ticks.is_multiple_of(12) {
            match chain.scan_orphans() {
                Ok(0) => {}
                Ok(n) => eprintln!("scan: {n} leftover encoded VAA(s) queued for closing"),
                Err(e) => eprintln!("scan: {e}"),
            }
        }
        ticks = ticks.wrapping_add(1);
        let mut all_done = true;
        let mut rate_limited = false;
        for window in &windows {
            let report = keeper.tick(&mut chain, &source, window);
            for s in chain.signatures().iter().skip(printed) {
                println!("{s}");
            }
            printed = chain.signatures().len();
            match report {
                Ok(r) => {
                    eprintln!(
                        "{window}: posted {}, already filled {}, skipped {}, finalized {:?}, done {}, open encoded VAAs {:?}",
                        r.posted,
                        r.already_filled,
                        r.skipped,
                        r.finalized,
                        r.done,
                        chain.orphans().iter().map(|o| o.encoded_vaa).collect::<Vec<_>>()
                    );
                    all_done &= r.done || r.finalized.is_some();
                }
                Err(e) => {
                    eprintln!("{window}: {e}");
                    rate_limited |= e == KeeperError::RateLimited;
                    all_done = false;
                }
            }
        }
        match args.watch {
            // Hermes' 429 asks for 60 s (`retry-after`); the key is shared with other keepers.
            Some(secs) if !all_done => {
                let wait = if rate_limited { secs.max(61) } else { secs };
                std::thread::sleep(Duration::from_secs(wait));
            }
            _ => return Ok(()),
        }
    }
}

fn main() -> ExitCode {
    match run(&Args::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
