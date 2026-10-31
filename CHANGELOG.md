# Changelog

All notable changes to the published crates are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the crates follow
[Semantic Versioning](https://semver.org/).

## [0.1.2] - 2026-10-31

`pyth-post` 0.1.1. The other crates are unchanged.

### pyth-post 0.1.1

- **Contract: one poster per payer.** Closing a leftover encoded VAA cannot be atomic against
  another poster's verify, so there is no guarantee between posters that share a payer. Scans,
  cleanup and posts for a payer must not overlap, even within one process: use a single poster
  (one `ReceiverChain`) per payer and do not share it across threads. `PayerLock` only prevents
  a second process.
- `PayerLock` enforces the contract. It takes an exclusive lock on `payer-<address>.lock` in
  `$PYTH_POST_LOCK_DIR` (default `~/.local/state/pyth-post`, a fixed path because a shell and a
  launchd agent can see different temporary directories). `default_lock_dir` and
  `LOCK_DIR_VAR` come with it.
- `LeftoverScanner` and `LEFTOVER_MIN_AGE` (300 blocks) are kept as defense in depth.
  - An encoded VAA is reported only once consecutive scans have seen its content (status and
    written bytes) unchanged for at least that many blocks. Every write, and the verify,
    restart the count.
  - The block height is read after the listing, so an account seen for the first time is
    never reported.
  - This keeps an unconditional periodic scan from closing a live post (the receiver's
    `WrongVaaOwner`, 6013, which metalbook hit).
- `scan_encoded_vaas` is unchanged. Its documentation now says it includes posts in flight.

### varswap-keeper (not published)

- The binary takes the `PayerLock` at start and refuses to run while another process holds
  it.
- `ReceiverChain::scan_orphans` adopts only what `LeftoverScanner` reports.

## [0.1.1] - 2026-10-30

Adds `pyth-post` 0.1.0 to crates.io. `pyth-lite`, `varswap-core` and `varswap-sdk` are unchanged
and stay at 0.1.0. The keeper and the CLI are still built from the repository.

### pyth-post 0.1.0

- `AccumulatorUpdate::parse` reads Hermes' binary update (`PNAU`): the VAA and every message
  with its merkle proof. `find` picks one feed's `PriceFeedMessage`.
- `ix` builds the Wormhole encoded VAA instructions (create, init, write,
  `verify_encoded_vaa_v1`, close) and the Pyth receiver's `post_update` and `reclaim_rent`.
- `post_plan` and `post` work over any `Transport` in three transactions. The first creates the
  encoded VAA and writes it, the second verifies it, and the third posts a `Full`
  `PriceUpdateV2`, runs the caller's instructions and closes both accounts. A post costs fees
  only, and every transaction fits the packet limit for up to 19 signatures.
- Recovery after a failed send: the encoded VAA is re-read and closed if it exists. An open one
  is returned as `PostError::Orphaned`. A first transaction that may still land is returned as
  `PostError::Uncertain`, with its last valid block height. `close_if_open` closes one later,
  and `scan_encoded_vaas` finds leftovers by write authority.
- `RpcTransport` is a `Transport` over `solana-rpc-client ~4.3`. Its URL is hidden from `Debug`
  and from errors.
- Tested in LiteSVM against the receiver and Wormhole programs dumped from devnet, with real
  Hermes updates, fault injection, and metalbook's receiver tests.

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
