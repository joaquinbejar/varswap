//! `varswap-admin`: see the crate documentation.

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_signer::Signer;
use varswap_cli::rpc::Cluster;
use varswap_cli::{
    config_json, create_window_ix, describe, feed_config_json, init_config_ix, parse_address,
    parse_feed_id, read_config, read_feed_config, read_keypair, read_swap, read_window,
    set_feed_config_ix, submit, swap_json, window_json, CliError,
};
use varswap_sdk::ix::FeedParams;
use varswap_sdk::pda::{self, WindowKey};
use varswap_sdk::PYTH_RECEIVER;

/// Admin commands and account reads for the varswap program.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// JSON-RPC URL. Never printed, so a provider key in it stays private.
    #[arg(long, env = "SOLANA_URL", global = true, hide_env_values = true)]
    url: Option<String>,
    /// The varswap program id.
    #[arg(long, env = "VARSWAP_PROGRAM_ID", global = true)]
    program_id: Option<String>,
    /// Keypair file of the signer (upgrade authority, admin or payer).
    #[arg(long, env = "VARSWAP_KEYPAIR", global = true)]
    keypair: Option<String>,
    /// Print the instruction instead of signing and sending it.
    #[arg(long, global = true)]
    dry_run: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// `init_config`: signed by the program's upgrade authority, who becomes admin. The
    /// collateral mint is immutable afterwards.
    InitConfig {
        /// The collateral mint (USDC).
        #[arg(long)]
        collateral_mint: String,
    },
    /// `set_feed_config`: creates or overwrites a feed's risk parameters; signed by the admin.
    SetFeedConfig {
        /// Pyth feed id, hex.
        #[arg(long)]
        feed: String,
        /// Pyth receiver program (default: the mainnet and devnet receiver).
        #[arg(long)]
        receiver: Option<String>,
        /// Smallest window interval, seconds.
        #[arg(long)]
        min_interval: u32,
        /// Outage guard, seconds (v1: 21600).
        #[arg(long)]
        tolerance: u32,
        /// Backfill period after a window's end, seconds (v1: 86400).
        #[arg(long)]
        backfill_deadline: u32,
        /// Largest number of buckets of a window, at most 1105.
        #[arg(long)]
        max_buckets: u32,
        /// Confidence bound, bps (v1: 1000).
        #[arg(long)]
        max_conf: u16,
        /// The feed's Pyth exponent (SOL/USD: -8).
        #[arg(long, allow_negative_numbers = true)]
        exponent: i8,
    },
    /// `create_window`: the signer pays its rent.
    CreateWindow {
        #[command(flatten)]
        key: Key,
    },
    /// The window address of a spec; reads nothing.
    WindowAddress {
        #[command(flatten)]
        key: Key,
    },
    /// Prints the config as JSON.
    ShowConfig,
    /// Prints a feed config as JSON.
    ShowFeed {
        /// Pyth feed id, hex.
        #[arg(long)]
        feed: String,
    },
    /// Prints a window as JSON.
    ShowWindow {
        /// The window address.
        address: String,
    },
    /// Prints a swap as JSON.
    ShowSwap {
        /// The swap address.
        address: String,
    },
}

/// The terms that name a window.
#[derive(clap::Args)]
struct Key {
    /// Pyth feed id, hex.
    #[arg(long)]
    feed: String,
    /// `b_0`, Unix seconds.
    #[arg(long)]
    start: i64,
    /// `b_N`, Unix seconds.
    #[arg(long)]
    end: i64,
    /// Grid spacing, seconds.
    #[arg(long)]
    interval: u32,
}

impl Key {
    fn key(&self) -> Result<WindowKey, CliError> {
        Ok(WindowKey {
            feed: parse_feed_id(&self.feed)?,
            start: self.start,
            end: self.end,
            interval: self.interval,
        })
    }
}

fn required<'a>(value: &'a Option<String>, what: &str) -> Result<&'a str, CliError> {
    value
        .as_deref()
        .ok_or_else(|| CliError::Usage(format!("{what} is required")))
}

fn print_json(value: &serde_json::Value) -> Result<(), CliError> {
    // `{:#}` is serde_json's pretty printer, which cannot fail on a `Value`.
    println!("{value:#}");
    Ok(())
}

/// Signs and sends `instruction`, or prints it with `--dry-run`.
fn finish(
    rpc: &mut Cluster,
    instruction: Instruction,
    signer: &Keypair,
    dry_run: bool,
) -> Result<(), CliError> {
    if dry_run {
        print!("{}", describe(&instruction));
    } else {
        let signature = submit(rpc, &[instruction], signer)?;
        println!("confirmed {signature}");
    }
    Ok(())
}

fn run(cli: Cli) -> Result<(), CliError> {
    let program_id = parse_address(required(&cli.program_id, "--program-id")?)?;
    if let Command::WindowAddress { key } = &cli.command {
        println!("{}", pda::window(&program_id, &key.key()?).0);
        return Ok(());
    }
    let mut rpc = Cluster::new(required(&cli.url, "--url")?);
    let signer = || read_keypair(required(&cli.keypair, "--keypair")?);
    match &cli.command {
        Command::ShowConfig => {
            let c = read_config(&mut rpc, &program_id)?;
            print_json(&config_json(&program_id, &c))
        }
        Command::ShowFeed { feed } => {
            let f = read_feed_config(&mut rpc, &program_id, &parse_feed_id(feed)?)?;
            print_json(&feed_config_json(&program_id, &f))
        }
        Command::ShowWindow { address } => {
            let address = parse_address(address)?;
            let (w, filled) = read_window(&mut rpc, &program_id, &address)?;
            print_json(&window_json(&address, &w, &filled))
        }
        Command::ShowSwap { address } => {
            let address = parse_address(address)?;
            let s = read_swap(&mut rpc, &program_id, &address)?;
            print_json(&swap_json(&address, &s))
        }
        Command::InitConfig { collateral_mint } => {
            let signer = signer()?;
            let instruction = init_config_ix(
                &program_id,
                &signer.pubkey(),
                &parse_address(collateral_mint)?,
            );
            finish(&mut rpc, instruction, &signer, cli.dry_run)
        }
        Command::SetFeedConfig {
            feed,
            receiver,
            min_interval,
            tolerance,
            backfill_deadline,
            max_buckets,
            max_conf,
            exponent,
        } => {
            let signer = signer()?;
            let p = FeedParams {
                feed: parse_feed_id(feed)?,
                receiver_program: receiver
                    .as_deref()
                    .map(parse_address)
                    .transpose()?
                    .unwrap_or(PYTH_RECEIVER),
                min_interval: *min_interval,
                tolerance: *tolerance,
                backfill_deadline: *backfill_deadline,
                max_buckets: *max_buckets,
                max_conf: *max_conf,
                exponent: *exponent,
            };
            let instruction = set_feed_config_ix(&program_id, &signer.pubkey(), &p);
            finish(&mut rpc, instruction, &signer, cli.dry_run)
        }
        Command::CreateWindow { key } => {
            let signer = signer()?;
            let k = key.key()?;
            println!("window {}", pda::window(&program_id, &k).0);
            let instruction = create_window_ix(&program_id, &signer.pubkey(), &k)?;
            finish(&mut rpc, instruction, &signer, cli.dry_run)
        }
        Command::WindowAddress { .. } => Ok(()),
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
