//! Compute units of every instruction, measured in LiteSVM.
//!
//! - Every scenario runs twice in fresh VMs and the counts must match (deterministic).
//! - With `VARSWAP_BENCH_OUT=<path>` the table is written there (`make bench` uses
//!   `docs/benchmarks.md`); it is always printed.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::disallowed_macros,
    clippy::result_large_err,
    missing_docs
)]

mod common;

use std::fmt::Write as _;
use std::path::PathBuf;

use common::*;

const START: i64 = T0 + H as i64;

fn b(k: u32) -> i64 {
    START + i64::from(H) * i64::from(k)
}

fn at(k: u32, price: i64) -> PriceUpdate {
    PriceUpdate::sol(price, b(k) + 1, b(k) - 1)
}

fn cu(r: Res) -> u64 {
    match r {
        Ok(m) => m.compute_units_consumed,
        Err(e) => panic!("{:?}\n{}", e.err, e.meta.pretty_logs()),
    }
}

/// A feed config allowing `max_buckets`, and a window of `n` hourly intervals.
fn window(n: u32, max_buckets: u32) -> (Env, solana_address::Address, u64) {
    let mut env = Env::new();
    env.init_config().unwrap();
    let mut a = feed_args(SOL);
    a.max_buckets = max_buckets;
    env.set_feed(a).unwrap();
    let end = b(n);
    let c = cu(env.create_window(SOL, START, end, H));
    (env, window_pda(SOL, START, end, H), c)
}

struct Row {
    name: &'static str,
    note: &'static str,
    run: fn() -> u64,
}

const ROWS: &[Row] = &[
    Row {
        name: "init_config",
        note: "find_program_address, ProgramData gate, create 40 B",
        run: || cu(Env::new().init_config()),
    },
    Row {
        name: "set_feed_config (create)",
        note: "find_program_address, create 96 B",
        run: || {
            let mut env = Env::new();
            env.init_config().unwrap();
            cu(env.set_feed(feed_args(SOL)))
        },
    },
    Row {
        name: "set_feed_config (update)",
        note: "re-derive with stored bump",
        run: || {
            let mut env = Env::new();
            env.init_config().unwrap();
            env.set_feed(feed_args(SOL)).unwrap();
            cu(env.set_feed(feed_args(SOL)))
        },
    },
    Row {
        name: "create_window (B = 25)",
        note: "1 day hourly, 369 B",
        run: || window(24, MAX_BUCKETS).2,
    },
    Row {
        name: "create_window (B = 721)",
        note: "30 days hourly, 6,729 B",
        run: || window(720, MAX_BUCKETS).2,
    },
    Row {
        name: "create_window (B = 1,105)",
        note: "MAX_BUCKETS, 10,233 B (CPI cap 10,240)",
        run: || window(1_104, varswap_core::MAX_BUCKETS).2,
    },
    Row {
        name: "sample (no neighbour)",
        note: "B = 721, first bucket, no kernel call",
        run: || {
            let (mut env, w, _) = window(720, MAX_BUCKETS);
            env.set_time(b(1));
            cu(env.sample(&w, &at(0, 15_000_000_000)))
        },
    },
    Row {
        name: "sample (both neighbours)",
        note: "B = 721, bucket 360 between two filled: 2 kernel calls",
        run: || {
            let (mut env, w, _) = window(720, MAX_BUCKETS);
            env.set_time(b(400));
            env.sample(&w, &at(359, 15_000_000_000)).unwrap();
            env.sample(&w, &at(361, 15_100_000_000)).unwrap();
            cu(env.sample(&w, &at(360, 14_900_000_000)))
        },
    },
    Row {
        name: "sample (outage run of 6)",
        note: "B = 721, one print fills 6 buckets (the 6 h guard maximum), 2 kernel calls",
        run: || {
            let (mut env, w, _) = window(720, MAX_BUCKETS);
            env.set_time(b(400));
            env.sample(&w, &at(100, 15_000_000_000)).unwrap();
            env.sample(&w, &at(107, 15_100_000_000)).unwrap();
            cu(env.sample(
                &w,
                &PriceUpdate::sol(14_900_000_000, b(106) + 5, b(101) - 10),
            ))
        },
    },
    Row {
        name: "finalize (complete)",
        note: "B = 25, all filled: annualize",
        run: || {
            let (mut env, w, _) = window(24, MAX_BUCKETS);
            env.set_time(b(24) + 2);
            for k in 0..=24 {
                env.sample(&w, &at(k, 15_000_000_000 + i64::from(k) * 1_000_000))
                    .unwrap();
            }
            cu(env.finalize(&w))
        },
    },
    Row {
        name: "finalize (degraded)",
        note: "B = 721, empty after the deadline",
        run: || {
            let (mut env, w, _) = window(720, MAX_BUCKETS);
            env.set_time(b(720) + i64::from(BACKFILL) + 1);
            cu(env.finalize(&w))
        },
    },
];

fn measure(row: &Row) -> u64 {
    let a = (row.run)();
    let b = (row.run)();
    assert_eq!(a, b, "{}: non-deterministic CU", row.name);
    a
}

#[test]
fn cu_table() {
    let no_events = std::env::var("VARSWAP_NO_EVENTS_SO")
        .ok()
        .map(PathBuf::from);
    let mut out = String::new();
    writeln!(out, "# Benchmarks").unwrap();
    writeln!(out).unwrap();
    writeln!(
        out,
        "Compute units per instruction, measured in LiteSVM 0.18 by `make bench` \
         (`program/tests/cu.rs`); each scenario runs twice in fresh VMs and must match."
    )
    .unwrap();
    if let Ok(tc) = std::env::var("VARSWAP_SBF_TOOLCHAIN") {
        writeln!(out, "Toolchain: {tc}.").unwrap();
    }
    writeln!(out).unwrap();
    writeln!(
        out,
        "`sample` runs `varswap_core::LnKernel` (two optfix `ln` calls per squared log \
         return). \"Without events\" is the same program built with `--no-default-features` (no \
         `sample` event log); the event column is the difference. PDA creation costs vary with \
         the number of bumps `find_program_address` tries for that address."
    )
    .unwrap();
    writeln!(out).unwrap();
    writeln!(
        out,
        "| Instruction | CU | Without events | Event log | Scenario |"
    )
    .unwrap();
    writeln!(out, "| --- | ---: | ---: | ---: | --- |").unwrap();
    for row in ROWS {
        use_program(program_path());
        let with = measure(row);
        let (without, event) = match &no_events {
            Some(p) => {
                use_program(p.clone());
                let n = measure(row);
                (n.to_string(), (with - n).to_string())
            }
            None => ("n/a".into(), "n/a".into()),
        };
        writeln!(
            out,
            "| `{}` | {with} | {without} | {event} | {} |",
            row.name, row.note
        )
        .unwrap();
    }
    use_program(program_path());
    println!("{out}");
    if let Ok(path) = std::env::var("VARSWAP_BENCH_OUT") {
        std::fs::write(path, out).unwrap();
    }
}
