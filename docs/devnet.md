# Devnet deployment

varswap runs on Solana devnet at the addresses below, deployed on 2026-10-30 from `main` at
`aef8e3a`. What is live: the program, `Config`, the SOL/USD `FeedConfig` and one hourly
window. Not live yet: sampling. The keeper's Hermes source needs the API key, and the
production RPC `Chain` (posting the print through the Pyth receiver, then `sample`) comes
with it.

## Addresses

| What | Address |
| --- | --- |
| Program | `BWqsaXvASrbErApP1ovz7vSvBLYqXGaMaZANEYf1xrB9` |
| ProgramData | `E5adgeUSG4tqpWMCU5y49kVskY6XEEfYMkh88e8GdF2g` |
| Upgrade authority, admin and payer | `77ieYLKspRnGzAt1AK6ZVsttnTY6T7EY2k7CpTZeCPPV` |
| `Config` (`["config"]`) | `H2H2gn4mwkWoJ8ftQf97hpKJuunRzhuVPn84ZzW58uLB` |
| Collateral mint (Circle devnet USDC, SPL Token, 6 decimals) | `4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU` |
| SOL/USD `FeedConfig` (`["feed", feed_id]`) | `85wokHgbLDTDqNeWhgBgD3hu2ZBM3r8qc1UnCUFixmEF` |
| Pyth receiver (pinned in the feed config) | `rec5EKMGg6MxZYaMdyBfgwp4d5rB9T1VQH5pJv5LtFJ` |
| Window (SOL/USD, 2026-10-31 hourly) | `7s5KJ79XRwBzeL8t42ut6RkQ1yLhrRMg8z5dHQtNXTeA` |

The program id keypair was generated with `solana-keygen` and stays on the deployer's
machine; it is not in the repository. The program is upgradeable (loader v3), which
`init_config` requires: it reads the upgrade authority from `ProgramData`.

## Build

`make program` (sBPF v3; SIMD-0178/0189/0377 is active on devnet since epoch 1069) with
`cargo-build-sbf 4.5.0`, platform-tools v1.57, Rust 1.95.0. `varswap_program.so` is 90,824
bytes, sha256 `10d702e5bdfa7a7172d5d464f7710012e495a0f7cf37cbeae401fba5c3d211bf`. The bytes
dumped from devnet (`solana program dump`, cut to the ELF length) have the same hash.

## Transactions

| Step | Signature |
| --- | --- |
| Deploy (`solana program deploy`, upgradeable loader) | `61QMuwr8CdHRWdATByumD2wNQFQWhBXKVAxhGv1NakYYkcDiPZqyhcKA7G7pW8M3Kx6SqfyQrQJx5eFAbzh5zmZo` |
| `init_config` (signed by the upgrade authority, which becomes admin) | `5HV3i3dFWJMKPZBMJxViWpsjpB6GkdzQUp8Vg1SH4duRyKL1JsjsgHGgCa5hA7dFugvpEuMhRFcrTPvGsXLeQaEf` |
| `set_feed_config` (SOL/USD) | `5c2ssG2VB53XiugwEKNXbQVWRtTbyNKwZBAb979yrZXE6DMijLtHkSpcJxzLtDdwLUBXnvDi2PiiAG9t95SWaCDm` |
| `create_window` | `4sudzjBz4ZU6Bw5QNq9pVJPhVUoyoaN3mg2gPMW36GKL5PymMeS5MWeSbs6UJiFR8tKBF9Wi44MhEr6UwtRJMdY9` |

The whole setup cost about 1.11 SOL. Most of it is the `ProgramData` rent; the deploy
buffer is refunded.

## Parameters

SOL/USD feed id `ef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d`:

| Parameter | Value | Why |
| --- | ---: | --- |
| `min_interval` | 60 s | allows short windows for live tests; the error bound at 60 s is in `docs/PLAN.md` 4.4 |
| `tolerance` | 21,600 s | the 6 h outage guard, v1 (`docs/PLAN.md` 4.1) |
| `backfill_deadline` | 86,400 s | the 24 h backfill, v1 |
| `max_buckets` | 1,105 | `MAX_BUCKETS`, the 10,240-byte creation cap |
| `max_conf` | 1,000 bps | v1 |
| `exponent` | -8 | SOL/USD's Pyth exponent |

The window is `start = 1793404800` (2026-10-31T00:00:00Z), `end = 1793491200`
(2026-11-01T00:00:00Z) and `interval = 3600`: 25 buckets, the 24-return SOL/USD replay of
milestone 6. Its backfill deadline is 2026-11-02T00:00:00Z. A keeper started before then can
still fill it from Hermes history; after it the window can only finalize as degraded.

## Checks on devnet

- `show-config`, `show-feed` and `show-window` read back exactly the values above. The
  window has 25 missing buckets and no realized variance.
- `set_feed_config` signed by a fresh funded key (`DZ7v368rF141E5LQ5zMMMK4h8LbogDLNNSNog7CSEeNe`)
  fails in simulation with custom error `0x6d` = 109 `NotAuthority`.
- `init_config` signed by the same key fails with 109 as well: the `ProgramData` gate runs
  before the existing-config check.

## Reproduce

```sh
export SOLANA_URL=https://api.devnet.solana.com
export VARSWAP_PROGRAM_ID=BWqsaXvASrbErApP1ovz7vSvBLYqXGaMaZANEYf1xrB9
export VARSWAP_KEYPAIR=~/.config/solana/id.json   # the upgrade authority

make program
solana program deploy --url devnet --upgrade-authority "$VARSWAP_KEYPAIR" \
  --program-id <program-id-keypair> target/deploy/varswap_program.so

varswap-admin init-config --collateral-mint 4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU
varswap-admin set-feed-config --feed ef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d \
  --min-interval 60 --tolerance 21600 --backfill-deadline 86400 --max-buckets 1105 \
  --max-conf 1000 --exponent -8
varswap-admin create-window --feed ef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d \
  --start 1793404800 --end 1793491200 --interval 3600
varswap-admin show-window 7s5KJ79XRwBzeL8t42ut6RkQ1yLhrRMg8z5dHQtNXTeA
```

Every write command takes `--dry-run` to print the instruction without sending it.

## Next

These steps need the Hermes API key (`VARSWAP_HERMES_API_KEY`, environment only):

1. The production RPC `Chain`: post each canonical print through the receiver
   (`post_update`), then call `sample`.
2. Run the keeper on the window above until it finalizes, and record the signatures here.
3. Commit 24 real canonical SOL/USD prints from Hermes as fixtures, and show that their
   replay finalizes to the off-chain value.
