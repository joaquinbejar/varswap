//! `varswap-watch`: a read-only monitor for varswap windows and keeper payers
//! (`docs/PLAN-improvements.md`, I6). It holds no key and queries no price source: it reads
//! the cluster only, so it keeps working, and alerting, when a keeper is dead.
//!
//! Each pass prints one JSON line per window and per payer, then runs `--alert-cmd` for every
//! alert that is new or has stayed active for `--realert` seconds, with the alert in the
//! environment (`VARSWAP_ALERT_KIND`, `VARSWAP_ALERT_SUBJECT`, `VARSWAP_ALERT_MESSAGE`; nothing
//! secret is ever on the command line). An alert that clears prints a `resolved` line.
//!
//! Checks: a due bucket empty for more than `--grace` seconds (`lag`), empty buckets within
//! `--deadline-warning` seconds of the backfill deadline (`deadline`), a window that can be
//! finalized and is not (`unfinalized`), a degraded window (`degraded`), a payer under
//! `--min-balance` lamports (`balance`), and an encoded VAA of a payer open for more than
//! `--vaa-alert-blocks` blocks (`encoded-vaa`).
//!
//! Configuration: `SOLANA_URL` (never printed) and `VARSWAP_PROGRAM_ID`.

use std::process::{Command, ExitCode};
use std::time::Duration;

use clap::Parser;
use serde_json::json;
use varswap_keeper::watch::{assess_window, Alert, AlertBook, Thresholds, VaaTracker};
use varswap_keeper::{RpcTransport, Transport};
use varswap_sdk::{decode, Address};

#[derive(Parser)]
#[command(version)]
struct Args {
    #[arg(long, env = "SOLANA_URL", hide_env_values = true)]
    url: String,
    #[arg(long, env = "VARSWAP_PROGRAM_ID")]
    program_id: String,
    /// A keeper payer to check (balance, open encoded VAAs); repeatable
    #[arg(long = "payer", value_name = "ADDRESS")]
    payers: Vec<String>,
    /// Seconds a due bucket may stay empty before `lag` fires
    #[arg(long, value_name = "SECONDS", default_value_t = 600)]
    grace: i64,
    /// Seconds before the backfill deadline at which empty buckets fire `deadline`
    #[arg(long, value_name = "SECONDS", default_value_t = 21_600)]
    deadline_warning: i64,
    /// Lamports under which a payer fires `balance`
    #[arg(long, value_name = "LAMPORTS", default_value_t = 50_000_000)]
    min_balance: u64,
    /// Blocks an encoded VAA may stay open before `encoded-vaa` fires (the keeper's scan runs
    /// every 12 ticks: an hour under the agent, about 9,000 blocks)
    #[arg(long, value_name = "BLOCKS", default_value_t = 12_000)]
    vaa_alert_blocks: u64,
    /// Shell command run for each alert, with the alert in VARSWAP_ALERT_* variables
    #[arg(long, value_name = "COMMAND", env = "VARSWAP_WATCH_ALERT_CMD")]
    alert_cmd: Option<String>,
    /// Repeat every SECONDS (default: one pass)
    #[arg(long, value_name = "SECONDS")]
    interval: Option<u64>,
    /// Seconds before an alert that stays active fires again
    #[arg(long, value_name = "SECONDS", default_value_t = 3_600)]
    realert: i64,
    #[arg(required = true)]
    windows: Vec<String>,
}

fn address(s: &str) -> Result<Address, String> {
    s.parse()
        .map_err(|_| format!("{s} is not a base58 address"))
}

/// `getBalance`, with the RPC URL never in an error.
fn balance(url: &str, payer: &Address) -> Result<u64, String> {
    let body =
        json!({"jsonrpc": "2.0", "id": 1, "method": "getBalance", "params": [payer.to_string()]});
    let mut response = ureq::post(url)
        .header("Content-Type", "application/json")
        .send(body.to_string())
        .map_err(|_| "getBalance failed (RPC URL hidden)".to_string())?;
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|_| "getBalance: unreadable answer".to_string())?;
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|_| "getBalance: not JSON".to_string())?;
    v.pointer("/result/value")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "getBalance: no value".to_string())
}

fn fire(cmd: Option<&str>, a: &Alert) {
    println!(
        "{}",
        json!({"alert": a.kind, "subject": a.subject, "message": a.message})
    );
    let Some(cmd) = cmd.filter(|c| !c.trim().is_empty()) else {
        return;
    };
    let status = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .env("VARSWAP_ALERT_KIND", a.kind)
        .env("VARSWAP_ALERT_SUBJECT", &a.subject)
        .env("VARSWAP_ALERT_MESSAGE", &a.message)
        .status();
    match status {
        Ok(s) if s.success() => {}
        Ok(s) => eprintln!("alert command exited with {s}"),
        Err(e) => eprintln!("alert command failed to start: {e}"),
    }
}

fn pass(
    args: &Args,
    program_id: &Address,
    windows: &[Address],
    payers: &[Address],
    chain: &RpcTransport,
    vaas: &mut VaaTracker,
) -> Result<(i64, Vec<Alert>), String> {
    let now = chain.now().map_err(|e| e.to_string())?;
    let height = chain.block_height().map_err(|e| e.to_string())?;
    let t = Thresholds {
        grace_s: args.grace,
        deadline_warning_s: args.deadline_warning,
    };
    let mut alerts = Vec::new();
    for w in windows {
        let name = w.to_string();
        let read = chain
            .account(w)
            .map_err(|e| e.to_string())
            .and_then(|a| a.ok_or_else(|| "account missing".to_string()))
            .and_then(|(owner, data)| {
                let info = decode::window(program_id, &owner, &data)
                    .map_err(|e| format!("not a window: {e:?}"))?;
                let filled = decode::filled_buckets(program_id, &owner, &data)
                    .map_err(|e| format!("not a window: {e:?}"))?;
                Ok((info, filled))
            });
        match read.and_then(|(info, filled)| {
            assess_window(&name, now, &info.spec, info.state, &filled, t).map_err(|e| e.to_string())
        }) {
            Ok((status, found)) => {
                println!("{}", json!({"window": name, "now": now, "status": status}));
                alerts.extend(found);
            }
            Err(e) => {
                println!("{}", json!({"window": name, "now": now, "error": e}));
                alerts.push(Alert {
                    kind: "unreadable",
                    subject: name.clone(),
                    message: format!("window {name} cannot be read: {e}"),
                });
            }
        }
    }
    for p in payers {
        let name = p.to_string();
        let lamports = balance(&args.url, p);
        let open = pyth_post::scan_encoded_vaas(chain, p)
            .map(|v| v.iter().map(ToString::to_string).collect::<Vec<_>>())
            .map_err(|e| e.to_string());
        println!(
            "{}",
            json!({
                "payer": name, "now": now, "block_height": height,
                "lamports": lamports.as_ref().ok(), "balance_error": lamports.as_ref().err(),
                "open_encoded_vaas": open.as_ref().ok(), "scan_error": open.as_ref().err(),
            })
        );
        if let Ok(l) = lamports {
            if l < args.min_balance {
                alerts.push(Alert {
                    kind: "balance",
                    subject: name.clone(),
                    message: format!(
                        "payer {name} has {l} lamports, under {}: top it up",
                        args.min_balance
                    ),
                });
            }
        }
        if let Ok(open) = open {
            alerts.extend(vaas.observe(&name, &open, height, args.vaa_alert_blocks));
        }
    }
    Ok((now, alerts))
}

fn run(args: &Args) -> Result<(), String> {
    let program_id = address(&args.program_id)?;
    let windows = args
        .windows
        .iter()
        .map(|w| address(w))
        .collect::<Result<Vec<_>, _>>()?;
    let payers = args
        .payers
        .iter()
        .map(|p| address(p))
        .collect::<Result<Vec<_>, _>>()?;
    let chain = RpcTransport::new(&args.url);
    let mut vaas = VaaTracker::default();
    let mut book = AlertBook::default();
    // Its own book, so a failed pass does not resolve the cluster alerts still active.
    let mut rpc_book = AlertBook::default();
    loop {
        match pass(args, &program_id, &windows, &payers, &chain, &mut vaas) {
            Ok((now, active)) => {
                let _ = rpc_book.update(&[], now, args.realert);
                let (to_fire, resolved) = book.update(&active, now, args.realert);
                for (kind, subject) in resolved {
                    println!("{}", json!({"resolved": kind, "subject": subject}));
                }
                for a in &to_fire {
                    fire(args.alert_cmd.as_deref(), a);
                }
            }
            Err(e) => {
                // The cluster cannot be read: that is an alert too (the clock is the host's).
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .and_then(|d| i64::try_from(d.as_secs()).ok())
                    .unwrap_or(0);
                let a = Alert {
                    kind: "rpc",
                    subject: "cluster".to_string(),
                    message: format!("varswap-watch cannot read the cluster: {e}"),
                };
                let (to_fire, _) = rpc_book.update(std::slice::from_ref(&a), now, args.realert);
                for a in &to_fire {
                    fire(args.alert_cmd.as_deref(), a);
                }
            }
        }
        match args.interval {
            Some(s) => std::thread::sleep(Duration::from_secs(s)),
            None => return Ok(()),
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
