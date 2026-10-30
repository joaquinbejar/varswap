# Changelog

All notable changes to the published crates are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the crates follow
[Semantic Versioning](https://semver.org/).

## [0.1.0] - 2026-10-30

First release on crates.io: `pyth-lite`, `varswap-core` and `varswap-sdk`. The program, the
keeper and the admin CLI are not published; they are built from the repository. The program
runs on devnet only (`docs/devnet.md`).

### pyth-lite

- `no_std` reader for Pyth `PriceUpdateV2` accounts with no Anchor and no Solana dependency.
  It checks the owner and the discriminator, decodes the layout by offset (`Partial` and
  `Full`), and verifies `Full` verification, feed id, positive price, staleness and
  confidence.
- The layout is cross-checked against pyth-solana-receiver-sdk 2.0.0's Anchor deserializer on
  real mainnet accounts.

### varswap-core

- Window grid and canonical print rule: `prev_publish_time < b_k <= publish_time <= b_k +
  tolerance`. A 6 h outage guard and a 24 h backfill are the v1 settings.
- `accept_print`: the acceptance rule shared by the program and the keeper.
- `LnKernel`: the squared log return on optfix 0.1 `ln` (Q64.64), with `sum_sq` at scale
  2^-96. The error bound is `ε = ceil(2δr√(cap·Y)/2^32) + ceil(3Y/2^32) + 2`, and payouts
  settle on `V_cons = max(0, V − ε)`.
- `SwapTerms`: capped, fully collateralized variance swaps, with directed rounding at every
  step.
- `layout`: every account and payload layout of the program.

### varswap-sdk

- Instruction builders, PDAs and account decoders for every varswap instruction and account.
