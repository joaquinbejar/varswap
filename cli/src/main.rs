//! `varswap-admin`: see the crate documentation.

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_signer::Signer;
use varswap_cli::rpc::Cluster;
use varswap_cli::{
    claim_ix, close_swap_ix, config_json, create_window_ix, describe, feed_config_json,
    init_config_ix, parse_address, parse_feed_id, read_config, read_feed_config, read_keypair,
    read_swap, read_window, set_feed_config_ix, settle_swap_ix, submit, swap_json, window_json,
    CliError, Side,
};
use varswap_cli::{index, record};
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
    /// `settle_swap`: anyone, once the swap's window is final (the window is read from the
    /// swap); computes and stores both payouts, moves no tokens. The claim and close commands
    /// support SPL Token swaps only (the only kind the program opens).
    SettleSwap {
        /// The swap address.
        swap: String,
    },
    /// `claim_long`: anyone; pays the long side into its token account of the swap's mint.
    ClaimLong {
        /// The swap address.
        swap: String,
        /// The long's token account (default: its associated token account of the mint).
        #[arg(long)]
        dest: Option<String>,
    },
    /// `claim_short`: anyone; pays the short side into its token account of the swap's mint.
    ClaimShort {
        /// The swap address.
        swap: String,
        /// The short's token account (default: its associated token account of the mint).
        #[arg(long)]
        dest: Option<String>,
    },
    /// `close_swap`: anyone, after both claims; rents and any vault balance go to the payer
    /// recorded in the swap.
    CloseSwap {
        /// The swap address.
        swap: String,
        /// The payer's token account of the mint (default: its associated token account).
        #[arg(long)]
        receiver_token: Option<String>,
    },
    /// Prints a window's record as JSON: account bytes, every transaction back to
    /// `create_window`, the derived fields and the quality label (finalized commitment).
    ExportWindow {
        /// The window address.
        address: String,
        /// The cluster's name, written into the record.
        #[arg(long, default_value = "devnet")]
        cluster: String,
    },
    /// Verifies a window record: offline, its consistency with itself (no chain
    /// authentication); with --rpc, also against the cluster.
    VerifyWindow {
        /// The record file.
        file: String,
        /// Also fetch the account and every transaction again and compare.
        #[arg(long)]
        rpc: bool,
    },
    /// Prints the markdown table rows of verified records (docs/windows/README.md).
    WindowTable {
        /// Record files.
        #[arg(required = true)]
        files: Vec<String>,
    },
    /// Brings a directory of records up to date with a series (docs/PLAN-phase2.md, P1): one
    /// file per ended, final day, and `sync.json`. Reads only; paced, with bounded retries.
    SeriesSync {
        /// The series file (docs/index/series/).
        series: String,
        /// The records directory.
        #[arg(long)]
        out: String,
        /// Requests per second.
        #[arg(long, default_value_t = 1)]
        rate: u32,
        /// Writes nothing: re-reads every `missing` stub and fails if its account exists.
        #[arg(long)]
        check: bool,
    },
    /// Prints the realized volatility index of a series from its records. Offline: every record
    /// is verified against itself; nothing is read from a cluster.
    VolIndex {
        /// The series file (docs/index/series/).
        series: String,
        /// The records directory.
        #[arg(long)]
        records: String,
        /// The chain time to read at, like 2026-11-04T18:00:00Z (default: sync.json's).
        #[arg(long)]
        as_of: Option<String>,
        /// The day (default: the last day ended at as_of, not the last with a record).
        #[arg(long, conflicts_with = "history")]
        day: Option<u32>,
        /// One entry per day instead of one day's values.
        #[arg(long)]
        history: bool,
        /// JSON output.
        #[arg(long)]
        json: bool,
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

fn read_record(file: &str) -> Result<serde_json::Value, CliError> {
    let text =
        std::fs::read_to_string(file).map_err(|e| CliError::Usage(format!("{file}: {e}")))?;
    serde_json::from_str(&text).map_err(|e| CliError::Usage(format!("{file}: {e}")))
}

fn read_series(file: &str) -> Result<index::Series, CliError> {
    index::Series::parse(&read_record(file)?)
}

/// One period of a day, in one line.
fn value_line(v: &serde_json::Value) -> String {
    let text = |k: &str| v.get(k).and_then(serde_json::Value::as_str).unwrap_or("-");
    let number = |k: &str| v.get(k).and_then(serde_json::Value::as_u64).unwrap_or(0);
    let line = format!(
        "{:>3}d  sigma {:<20} rv {:<20} {:<12} usable {}/{}  missed {}  late {}",
        number("days"),
        text("sigma"),
        text("rv"),
        text("label"),
        number("usable"),
        number("days"),
        number("missed_samples"),
        number("late_samples"),
    );
    match number("late_unknown") {
        0 => line,
        unknown => format!("{line} (and {unknown} with no block time)"),
    }
}

fn vol_index(
    series: &str,
    records: &str,
    as_of: Option<&str>,
    day: Option<u32>,
    history: bool,
    as_json: bool,
) -> Result<(), CliError> {
    let series = read_series(series)?;
    let dir = std::path::Path::new(records);
    let as_of = index::resolve_as_of(dir, as_of.map(index::parse_time).transpose()?)?;
    if history {
        return print_json(&index::history(&series, dir, as_of)?);
    }
    let out = index::index_at(&series, dir, as_of, day)?;
    if as_json {
        return print_json(&out);
    }
    match out.get("day").and_then(serde_json::Value::as_u64) {
        None => println!("{}: no day has ended at as_of {as_of}", series.id),
        Some(d) => {
            let start = out.get("start").and_then(serde_json::Value::as_str);
            println!("{} day {d} (from {})", series.id, start.unwrap_or("?"));
            let values = out.get("values").and_then(serde_json::Value::as_array);
            for v in values.into_iter().flatten() {
                println!("{}", value_line(v));
                let excluded = v.get("excluded").and_then(serde_json::Value::as_array);
                for e in excluded.into_iter().flatten() {
                    println!(
                        "      excluded: day {} {}",
                        e.get("day").unwrap_or(&serde_json::Value::Null),
                        e.get("reason")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("?"),
                    );
                }
                let before = v.get("before_series").and_then(serde_json::Value::as_u64);
                if let Some(n) = before.filter(|n| *n > 0) {
                    println!("      {n} days before the series started");
                }
            }
        }
    }
    Ok(())
}

fn run(cli: Cli) -> Result<(), CliError> {
    // Offline commands: no cluster, no program id.
    match &cli.command {
        Command::VerifyWindow { file, rpc: false } => {
            let derived = record::verify_offline(&read_record(file)?)?;
            println!(
                "offline: consistent (the account bytes, the events and the derived fields agree; not authenticated against the chain): label {}",
                derived.get("label").and_then(serde_json::Value::as_str).unwrap_or("?")
            );
            return Ok(());
        }
        Command::WindowTable { files } => {
            println!("| Window | State | Coverage | Realized variance | Label | Compressed runs |");
            println!("| --- | --- | ---: | ---: | --- | ---: |");
            for f in files {
                let name = std::path::Path::new(f)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(f);
                println!("{}", record::table_row(&read_record(f)?, name)?);
            }
            return Ok(());
        }
        Command::VolIndex {
            series,
            records,
            as_of,
            day,
            history,
            json,
        } => return vol_index(series, records, as_of.as_deref(), *day, *history, *json),
        Command::SeriesSync {
            series,
            out,
            rate,
            check,
        } => {
            let series = read_series(series)?;
            let pace = index::Pace::per_second(*rate)?;
            let cluster = Cluster::new(required(&cli.url, "--url")?);
            let mut paced = index::Paced::new(cluster, pace, std::thread::sleep);
            let dir = std::path::Path::new(out);
            if *check {
                let checked = index::check_stubs(&mut paced, &series, dir)?;
                println!("{checked} missing stubs checked: each account is still absent");
                return Ok(());
            }
            return print_json(&index::series_sync(&mut paced, &series, dir)?);
        }
        _ => {}
    }
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
        Command::SettleSwap { swap } => {
            let instruction = settle_swap_ix(&mut rpc, &program_id, &parse_address(swap)?)?;
            finish(&mut rpc, instruction, &signer()?, cli.dry_run)
        }
        Command::ClaimLong { swap, dest } | Command::ClaimShort { swap, dest } => {
            let side = if matches!(cli.command, Command::ClaimLong { .. }) {
                Side::Long
            } else {
                Side::Short
            };
            let dest = dest.as_deref().map(parse_address).transpose()?;
            let instruction = claim_ix(
                &mut rpc,
                &program_id,
                &parse_address(swap)?,
                side,
                dest.as_ref(),
            )?;
            finish(&mut rpc, instruction, &signer()?, cli.dry_run)
        }
        Command::CloseSwap {
            swap,
            receiver_token,
        } => {
            let token = receiver_token.as_deref().map(parse_address).transpose()?;
            let instruction =
                close_swap_ix(&mut rpc, &program_id, &parse_address(swap)?, token.as_ref())?;
            finish(&mut rpc, instruction, &signer()?, cli.dry_run)
        }
        Command::ExportWindow { address, cluster } => {
            let address = parse_address(address)?;
            print_json(&record::export(&mut rpc, &program_id, &address, cluster)?)
        }
        Command::VerifyWindow { file, .. } => {
            let b = read_record(file)?;
            if b.get("program_id").and_then(serde_json::Value::as_str)
                != Some(program_id.to_string().as_str())
            {
                return Err(CliError::Usage(
                    "the record is for another program id".to_owned(),
                ));
            }
            let derived = record::verify_rpc(&mut rpc, &b)?;
            println!(
                "rpc: consistent and equal to the cluster at finalized commitment: label {}",
                derived
                    .get("label")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("?")
            );
            Ok(())
        }
        Command::WindowTable { .. } | Command::VolIndex { .. } | Command::SeriesSync { .. } => {
            Ok(())
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
