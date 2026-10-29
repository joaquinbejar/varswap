# Benchmarks

Compute units per instruction, measured in LiteSVM 0.18 by `make bench` (`program/tests/cu.rs`); each scenario runs twice in fresh VMs and must match.
Toolchain: cargo-build-sbf 4.5.0, Default platform-tools: v1.57, Default Rust: 1.95.0, sBPF v3.

Milestone 3a: `sample` and `finalize` run the exact integer stand-in kernel (`varswap_core::IntegerKernel`); their rows change when milestone 1b plugs in optfix `ln`. "Without events" is the same program built with `--no-default-features` (no `sample` event log); the event column is the difference. PDA creation costs vary with the number of bumps `find_program_address` tries for that address.

| Instruction | CU | Without events | Event log | Scenario |
| --- | ---: | ---: | ---: | --- |
| `init_config` | 7605 | 7605 | 0 | find_program_address, ProgramData gate, create 40 B |
| `set_feed_config (create)` | 4735 | 4735 | 0 | find_program_address, create 96 B |
| `set_feed_config (update)` | 3501 | 3501 | 0 | re-derive with stored bump |
| `create_window (B = 25)` | 7833 | 7833 | 0 | 1 day hourly, 369 B |
| `create_window (B = 721)` | 6861 | 6861 | 0 | 30 days hourly, 6,729 B |
| `create_window (B = 1,105)` | 7149 | 7149 | 0 | MAX_BUCKETS, 10,233 B (CPI cap 10,240) |
| `sample (no neighbour)` | 3413 | 2760 | 653 | B = 721, first bucket, no kernel call |
| `sample (both neighbours)` | 5172 | 4441 | 731 | B = 721, bucket 360 between two filled: 2 kernel calls |
| `sample (outage run of 6)` | 5822 | 5091 | 731 | B = 721, one print fills 6 buckets (the 6 h guard maximum), 2 kernel calls |
| `finalize (complete)` | 1073 | 1073 | 0 | B = 25, all filled: annualize |
| `finalize (degraded)` | 1252 | 1252 | 0 | B = 721, empty after the deadline |
