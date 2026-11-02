# varswap: improvement plan

Scope given by the SOLANA-PROJECTS coordinator (2026-11-02, from the roadmap v2 review): first
harden what exists (CI, verifiable build, upgrade authority, runbooks, audit scope, fuzzing),
then fix the README, write the Degraded policy ADR and a mainnet readiness document, add an
independent keeper with alerts, and publish realized variance windows reproducibly. No
mainnet action in any milestone. No new on chain feature.

Each milestone is one branch and one PR. `make pre-push` passes before every push, codex
reviews every PR before merge, and the coordinator gets a report at each merge. Global rules
apply (signed commits, English, checked arithmetic, no panics outside tests, `MAJOR.MINOR`
dependencies, no new dependency without approval).

State at the start (main `a7f47de`): no `.github/workflows`; the only gate is a local
`make pre-push` (`fmt-check lint lint-control sbf sbf-control test doc`). The devnet program
`BWqsaXvASrbErApP1ovz7vSvBLYqXGaMaZANEYf1xrB9` has one key, `77ie…`, as deployer, upgrade
authority and config admin. Property tests exist in `varswap-core` only (`tests/model.rs`,
`tests/swap.rs`, `tests/ln_kernel.rs`). Two devnet windows finalized (`docs/devnet.md`).

## Phase 1: hardening

### I1. CI as a required check

Port optfix milestone 9 (`optfix/.github/workflows/ci.yml`, `.github/actions/toolchains`).

- Same pins as optfix, same mechanism: Agave CLI 4.4.0 tarball and platform-tools v1.57
  archive, each checked against its recorded SHA-256; host Rust 1.99.1 with rustfmt and
  clippy; a step fails unless `cargo build-sbf --version` reports `cargo-build-sbf 4.5.0`,
  platform-tools `v1.57` and Rust `1.95.0` (the values `docs/benchmarks.md` records) and
  `rustc`, `rustfmt`, `clippy` report the host pin.
- Makefile: `+stable` becomes `+$(RUST_TOOLCHAIN)` (`stable` by default, the pin in CI) and
  every cargo call takes `$(CARGO_FLAGS)` (empty by default, `--locked` in CI). Local
  `make pre-push` is unchanged.
- Jobs on `pull_request` and pushes to `main`, `ubuntu-latest`, read-only token, no secrets,
  actions pinned by commit SHA:
  - `checks`: `fmt-check lint lint-control sbf sbf-control doc`, plus the optfix step that
    fails if the `pre-push` prerequisites change without the workflow;
  - `test`: `make program test` (one job; split into shards with full case counts only if
    it takes over 30 minutes, as in optfix);
  - `bench`: `make bench`, then `git diff --exit-code docs/benchmarks.md`.
- The tests are offline (LiteSVM, program dumps and Hermes fixtures in the repo); CI proves
  it, since runners have no Hermes key. If any test needs the network it is fixed, not
  skipped.
- Failing controls on scratch PRs closed unmerged: an unformatted line fails `checks` only;
  a planted failing test fails `test` only; one edited number in `docs/benchmarks.md` fails
  `bench` only.
- Then `main` branch protection requires `checks`, `test` and `bench` (coordinator's GO
  covers it). Badge in the README.
- *Done when* every job passes on the milestone PR, a CI-built benchmark table equals the
  committed one byte for byte, each failing control fails exactly its job, and the checks
  are required on `main`.

### I2. Verifiable build and a separate upgrade authority

- `make verifiable`: `solana-verify build` (the CLI installed with `cargo install --locked`,
  a tool, not a crate dependency) in the `solanafoundation/solana-verifiable-build` image
  pinned by digest, for `program/` with default features. It prints the executable hash.
- A CI job `verifiable` runs it and fails unless the hash equals the one recorded in
  `docs/devnet.md`, so a change to the program without a recorded hash fails CI.
- Devnet: upgrade `BWqs…` with the verifiable artifact; `solana-verify get-program-hash`
  on devnet equals the local and CI hash. Recorded in `docs/devnet.md` with the upgrade
  signature. The keeper and the 24 print replay are rerun against it (the build is
  equivalent, but the deployed bytes change).
- Upgrade authority: a dedicated devnet key (`~/.config/solana/varswap-devnet-upgrade.json`,
  never committed) takes it over with `solana program set-upgrade-authority`. `77ie…` stays
  the config admin, which is fixed at `init_config` (there is no admin rotation
  instruction, by design).
- `docs/governance.md`: roles (upgrade authority, config admin, keeper payers, Hermes key),
  who holds each on devnet today, and the mainnet plan: a Squads v4 multisig (threshold and
  members are the coordinator's call) is the upgrade authority before deployment and signs
  `init_config` itself, so the config admin is the multisig vault from the start and no
  rotation instruction is needed; upgrades only from a verifiable build whose hash the
  signers check; freezing (`--final`) after the audit is an explicit decision, not a default.
  No mainnet action.
- *Done when* the devnet program hash equals the CI hash, the upgrade authority is the new
  key, and a keeper plus the replay pass against the upgraded program.

### I3. Fuzzing and conservation

Targets (each must not panic, and each checks its invariants):

- `pyth_update`: arbitrary account bytes and owner into `pyth_lite::PriceUpdate::load`,
  `decode` and `verify`. An accepted account re-encodes to the same bytes in the fields
  read, and changing any one byte of discriminator, owner, verification level or feed id
  makes it rejected.
- `window_sample`: an arbitrary window spec and an arbitrary sequence of prints applied to
  a `varswap-core` window. A rejected print leaves the window byte for byte unchanged;
  `filled` equals the bitmap population; filled buckets never change; `sum_sq` equals a
  recomputation from the stored prices with the same kernel; `finalize` gives `Finalized`
  exactly when full and `Degraded` only after the deadline.
- `swap_conservation`: arbitrary `SwapTerms` and settlement. `long + short == total`
  exactly, both within `u64`, the long payout monotone in the realized variance and capped,
  and `Unwind` returns each collateral.
- `sample_ix` (program level, LiteSVM): arbitrary `PriceUpdateV2` account data owned by the
  receiver against a window. Either an error with the window account unchanged, or a state
  equal to applying the same print in `varswap-core`.

Harness: cargo-fuzz in a `fuzz/` crate outside the workspace (nightly pinned in CI only),
with seed corpora from the real fixtures (Hermes updates, the 24 replay prints, mainnet
`PriceUpdateV2` dumps). It needs `libfuzzer-sys` and `arbitrary`, which are new
dependencies: **asking the coordinator**. If refused, the same targets run as proptest
(already a workspace dev-dependency) with raised case counts, and `sample_ix` reuses the
program test harness. Either way the harness asserts by panicking, which is test code.

CI: a `fuzz` job runs each target 120 s on pull requests and 30 minutes on a weekly
schedule; a crash fails the job and uploads the input. Found crashes become regression
tests in the normal suite.

*Done when* each target runs clean for 30 minutes locally, a planted bug (dropping the
`checked` on one payout addition; accepting a `Partial` verification level) is found by
its target within the PR budget, and the planted bug is reverted.

### I4. Runbooks and audit scope

- `docs/runbooks/`, one file per incident, each with detection, commands and how to verify:
  keeper install, restart and uninstall (`scripts/keeper-agent.sh`); payer top-up; Hermes
  429 and key rotation (the key is shared with metalbook: about 10 requests per minute);
  orphaned encoded VAAs (`scan_orphans`, the 300 block guard); a window close to its
  backfill deadline with missing buckets (manual backfill with `varswap-keeper`); a
  `Degraded` window (settle and claims unwind); program upgrade (I2); suspected key
  compromise (payer, upgrade authority, Hermes key).
- Drills on devnet, each recorded with times: stop the keeper by PID mid window and
  restart it; force an orphan (kill between the first and second transaction) and clean it;
  backfill a missed hour.
- `docs/audit-scope.md`: in scope (`program`, `varswap-core`, `pyth-lite`, with line
  counts at a pinned commit and the verifiable hash), separately scoped off chain code
  (`pyth-post`, `keeper`), out of scope (CLI, study); trust assumptions (Pyth publishers,
  Wormhole guardians, the receiver program and its config); invariants with the test or
  fuzz target that checks each; known limitations (the Degraded policy, the 6 h guard,
  publish time collisions); prior reviews (codex PR reviews with links).
- *Done when* each runbook has been followed once end to end on devnet (the
  compromise runbook as a dry run) and codex has checked the invariant table against code.

## Phase 2

### I5. README, Degraded ADR, mainnet readiness

- README: the Status section still says the devnet run remains, live sampling waits on the
  Hermes key and no crate is published (lines 55 to 56), while Install lists four crates on
  crates.io (lines 94 to 95) and `docs/devnet.md` records two finalized windows. The title
  says v0.1.0 while `varswap-core` and `varswap-sdk` are 0.2.0. Rewrite Status and the title
  against the facts, and link CI, the verifiable hash and the plans.
- `docs/adr/0001-degraded-unwind.md`: context (`varswap-core/src/window.rs:436-443`:
  `Degraded` when a bucket is empty after the deadline; `program/src/swap.rs:242-249`:
  `Degraded` settles as `Unwind`); the evidence (`docs/study.md`: 30 day hourly windows at
  σ = 80%, 64.4% unwind with a 1 h guard, 12.4% with 3 h, 0.62% with 6 h, under the assumed
  outage regime); alternatives (coverage thresholds, interpolation, scaling by coverage,
  longer deadlines, per feed guards, partial settlement), each with why it does or does not
  bound the pathwise error; the decision for v1; consequences; what would reopen it.
- `docs/mainnet-readiness.md`: a checklist with an owner and evidence per line: mainnet
  receiver, Wormhole and guardian set addresses verified; feed sponsorship and update
  cadence checked; a dedicated Hermes key; multisig in place (I2); audit done (I4 scope);
  CI and verifiable hash; runbooks drilled; monitoring (I6); collateral mint and feed
  parameters; launch limits.
- *Done when* every claim in the README is checked against the code or docs, and codex
  approves the ADR's citations.

### I6. Independent keeper with alerts

- `varswap-watch` (a binary in `keeper`): read-only RPC, no keys, no Hermes. Per window:
  the lag of the next due bucket against `boundary + grace`, buckets still missing against
  the backfill deadline, and the state; per configured payer: balance and open encoded VAAs
  (`scan_encoded_vaas`, read-only). One JSON line per check; an alert runs
  `--alert-cmd` with the alert in environment variables (no secret in the command line).
  It runs as its own LaunchAgent, so it notices a dead keeper.
- A standby keeper: `varswap-keeper --standby-lag S` only samples buckets whose boundary is
  older than `S` (default 900 s), with its own payer (the contract: one poster per payer)
  and its own lock. It queries Hermes only when the primary lags, so it costs no Hermes
  budget while the primary is healthy.
- Hermes key: a standby on the same shared key still competes with metalbook during an
  incident. **Asking the coordinator** whether a second key exists; if not, the standby
  backs off under the same 429 rules and the runbook says so.
- Drill on devnet: a new 24 bucket hourly window with primary, standby and watcher; stop the
  primary by PID for 3 hours. The watcher alerts within one interval plus grace, the standby
  fills every bucket, the window finalizes, and no encoded VAA is left open.
- *Done when* the drill window finalizes complete with the alert times recorded in
  `docs/devnet.md`.

### I7. Reproducible publication of realized variance windows

Not a continuous index: one record per finalized or degraded window.

- `varswap-admin export-window <window>`: a JSON record with the spec, feed, state,
  coverage (filled over buckets), the per bucket prices and exponents read from the account,
  the per sample events (`varswap:sample first last publish_time price expo`, from the
  window's transaction logs), `sum_sq` and the realized variance.
- `varswap-admin verify-window <record>`: recomputes `sum_sq` and the realized variance from
  the record's prices with `varswap-core` and checks them, and the prices, against the
  account; exit code nonzero on any difference.
- Quality label from the events: `complete` (every bucket from its own print), `compressed`
  (complete, with outage runs: count and longest run in buckets, at most the guard), or
  `degraded` (no realized variance; coverage shown).
- `docs/windows/`: one JSON per window plus a README table generated by the CLI, starting
  with the demo window and `7s5K`. A CI step reruns `verify-window` on the committed records
  offline (the account bytes are committed alongside).
- *Done when* both devnet windows are published, `verify-window` passes on each against
  devnet and offline, and a record with one edited price fails.

## Questions for the coordinator

1. `libfuzzer-sys` and `arbitrary` for `fuzz/` (I3), or proptest only.
2. A second Hermes key for the standby keeper (I6).
3. Multisig threshold and members for the mainnet plan (I2), or leave them open in the
   document.
