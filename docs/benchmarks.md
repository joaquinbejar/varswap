# Benchmarks

Compute units per instruction, measured in LiteSVM 0.18 by `make bench` (`program/tests/cu.rs`); each scenario runs twice in fresh VMs and must match.
Toolchain: cargo-build-sbf 4.5.0, Default platform-tools: v1.57, Default Rust: 1.95.0, sBPF v3.

`sample` runs `varswap_core::LnKernel` (two optfix `ln` calls per squared log return). "Without events" is the same program built with `--no-default-features` (no `sample` event log); the event column is the difference. PDA creation costs vary with the number of bumps `find_program_address` tries for that address.

| Instruction | CU | Without events | Event log | Scenario |
| --- | ---: | ---: | ---: | --- |
| `init_config` | 7632 | 7632 | 0 | find_program_address, ProgramData gate, create 40 B |
| `set_feed_config (create)` | 4765 | 4765 | 0 | find_program_address, create 96 B |
| `set_feed_config (update)` | 3533 | 3533 | 0 | re-derive with stored bump |
| `create_window (B = 25)` | 7851 | 7851 | 0 | 1 day hourly, 369 B |
| `create_window (B = 721)` | 6879 | 6879 | 0 | 30 days hourly, 6,729 B |
| `create_window (B = 1,105)` | 7167 | 7167 | 0 | MAX_BUCKETS, 10,233 B (CPI cap 10,240) |
| `sample (no neighbour)` | 3471 | 2816 | 655 | B = 721, first bucket, no kernel call |
| `sample (both neighbours)` | 6673 | 5940 | 733 | B = 721, bucket 360 between two filled: 2 kernel calls |
| `sample (outage run of 6)` | 7323 | 6590 | 733 | B = 721, one print fills 6 buckets (the 6 h guard maximum), 2 kernel calls |
| `finalize (complete)` | 1148 | 1148 | 0 | B = 25, all filled: annualize |
| `finalize (degraded)` | 1306 | 1306 | 0 | B = 721, empty after the deadline |
| `open_swap` | 16427 | 16427 | 0 | create swap 288 B and vault 165 B, initialize_account3, two transfer_checked |
| `settle_swap (finalized)` | 3018 | 3018 | 0 | inspect window, V_cons, payouts |
| `settle_swap (degraded)` | 2073 | 2073 | 0 | unwind: each side's collateral |
| `claim_long` | 3059 | 3059 | 0 | vault PDA check, transfer_checked signed by the swap PDA |
| `close_swap` | 2975 | 2975 | 0 | close_account on the vault, close the swap, rent to the payer |
