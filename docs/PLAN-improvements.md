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

### I1. CI on every pull request

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
- No branch protection on `main` (decision 6: required CI is for optfix only). The rule is
  a process rule instead: a PR merges only when `checks`, `test` and `bench` are green on
  its head (and `fuzz` once I3 adds it). Badge in the README.
- *Done when* every job passes on the milestone PR, a CI-built benchmark table equals the
  committed one byte for byte, and each failing control fails exactly its job.

### I2. Verifiable build and a separate upgrade authority

- `make verifiable`: `solana-verify build` (the CLI installed with `cargo install --locked`,
  a tool, not a crate dependency) in the `solanafoundation/solana-verifiable-build` image
  pinned by digest, for `program/` with default features. It prints the executable hash.
- Two hashes, kept apart:
  - the **build hash** of the current source, in `docs/verifiable.md` with the commit,
    the image digest and the toolchain. A CI job `verifiable` rebuilds it and fails unless it
    equals the recorded one, so a program change must update it in the same PR. It says
    what main would deploy, not what is deployed;
  - the **deployed hash**, in `docs/devnet.md`: the hash `solana-verify get-program-hash`
    reads on devnet, the commit it was built from, the upgrade signature and slot. It
    changes only with an upgrade and may lag main; the doc says so.
- Devnet: upgrade `BWqs…` with the verifiable artifact; the deployed hash then equals the
  build hash of that commit. The keeper and the 24 print replay are rerun against it (the
  build is equivalent, but the deployed bytes change). Pre-work: the image
  `solana-verifiable-build:4.3.0` bundles platform-tools v1.57 and Rust 1.95.0, the SBF
  compiler CI pins; its `.so` gives the same CU on every `docs/benchmarks.md` row and passes
  the 81 program tests.
- Upgrade authority: a dedicated devnet key (`~/.config/solana/varswap-devnet-upgrade.json`,
  mode 0600, never committed, with a backup copy outside the repo) takes it over with
  `solana program set-upgrade-authority` (decision 4: its pubkey and path go to the
  coordinator, and `77ie…` is kept until the transfer is confirmed on chain). `77ie…` stays
  the config admin, which is fixed at `init_config` (there is no admin rotation
  instruction, by design).
- `docs/governance.md`: roles (upgrade authority, config admin, keeper payers, Hermes key),
  who holds each on devnet today, and the mainnet plan: a Squads v4 multisig (threshold and
  members are the coordinator's call) is the upgrade authority before deployment and signs
  `init_config` itself, so the config admin is the multisig vault from the start and no
  rotation instruction is needed; upgrades only from a verifiable build whose hash the
  signers check; freezing (`--final`) after the audit is an explicit decision, not a default.
  No mainnet action.
- *Done when* CI's `verifiable` job reproduces the recorded build hash, the deployed hash on
  devnet equals the build hash of its recorded commit, the upgrade authority is the new key,
  and a keeper plus the replay pass against the upgraded program.

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
`PriceUpdateV2` dumps). It uses `libfuzzer-sys` and `arbitrary` (approved, decision 1);
`sample_ix` reuses the program test harness. The harness asserts by panicking, which is
test code.

CI: a `fuzz` job runs each target 120 s on pull requests and 30 minutes on a weekly
schedule; a crash fails the job and uploads the input. Found crashes become regression
tests in the normal suite.

Fault controls are deterministic, not a timed hope:

- each control is a committed patch under `fuzz/controls/` that plants one bug in the code
  under test (pyth-lite accepting a `Partial` level; a swap payout dropping its rounding up;
  `fill` skipping the "nothing written on error" rule; the program skipping the confidence
  check), plus the committed input that exposes it;
- `make fuzz-control` applies each patch in a scratch worktree and runs its target on that
  input with a fixed seed (`-seed=1`, `-runs=` the input count). The target must crash, and
  the same input must pass on the unpatched tree. CI runs it in the `fuzz` job.

*Done when* each target runs clean for 30 minutes locally and in the weekly job, and every
fault control crashes on its patch and passes without it, in CI.

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
  `Degraded` settles as `Unwind`); the evidence (`docs/study.md`, model GBM μ = 0: 30 day
  hourly windows at σ = 80%, 64.40% unwind with a 1 h guard, 12.40% with 3 h, 0.62% with
  6 h, under the assumed outage regime; the halt and AR(1) rows reported beside them);
  alternatives (coverage thresholds, interpolation, scaling by coverage,
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
  older than `S`, with its own payer (the contract: one poster per payer) and its own lock.
  `S` is validated: at least 600 s and at most the window interval minus 300 s (an hourly
  window: 600 to 3,300 s, default 900 s); anything else is a startup error, so `S` can
  never make pacing degenerate. While the primary is healthy the standby finds nothing due
  and sends no Hermes request.
- Hermes budget (decision 2: one key, shared with metalbook; R1 P2). Pacing is not inferred
  from `S`; it is an explicit allocation enforced where requests leave the process:
  - **Allocation per key** (decision 2): of the key's quota on the timestamp endpoint
    (about 10 requests per minute), varswap gets `R = 3` per minute for all its processes
    together and metalbook keeps 7; written in `docs/governance.md`. Hourly windows need
    one request per window per hour in steady state; a 24 bucket backfill at 3 per minute
    takes 8 minutes.
  - **One rolling-window limiter for every varswap process on one host**: a file under the
    lock directory (`$PYTH_POST_LOCK_DIR`, flock on every take), keyed by a hash of the API
    key (never the key itself), holding one entry per recent request. A request may leave
    only if fewer than `R` entries fall in the last 60 s; otherwise it waits. So at most `R`
    requests in any sliding minute (a refilled token bucket could let nearly `2R` through).
    `HermesSource` takes a slot before every HTTP request it sends, including retries and
    requests answered 404 or 429; the cache answers without one. No slot available is a
    wait, not an error.
  - **Amendment (review of #39; coordinator ruling): the contract is wire time.** Hermes
    counts the requests that reach it, so at most `R` requests may arrive in any sliding
    60 s, not merely be admitted. An entry recorded at admission does not bound arrival: a
    process paused between admission and send (the drill's SIGSTOP, or scheduling on a
    loaded host) sends after its entry has aged out, inside a window others have filled.
    So an admitted request is recorded as pending, and its process creates and `flock`s a
    lock file of its own for that request until it has been answered or has failed. A
    pending entry counts for as long as that lock is held, however long the process is
    stopped; when the request completes (`confirm`), or another process finds the lock free
    (the owner died), the entry becomes completed at that time and counts for one more
    window. So every entry counts from before its request can be sent until a window after
    it can have arrived: no `R + 1` arrivals within one window. A stopped process holds one
    slot until it resumes or dies. Widening test tolerances is not a remedy, and there is no
    global send lock: a hung owner costs its own slot only, so the standby keeps the rest.
    Pending reservations count with no time limit; a dead owner's (its lock released by the
    kernel, even after SIGKILL) is reaped by the next process to look, and counts one more
    window from then.
  - **All slots hung**: `varswap-watch` reads the limiter files (read only, no key) and
    raises `hermes-hung` when every slot has been held by a request in progress for longer
    than `--hermes-hung-after` (120 s); the runbook action is to kill the hung process by PID
    (`docs/runbooks/hermes.md`).
  - **Tests**: a send stalled 1.5 and 5 windows after admission while another process sends
    flat out; a holder stopped for 3 windows; across processes, a child SIGSTOPped while
    holding the slot (held, shown with its PID, `hermes-hung` raised) and then SIGKILLed
    (reaped, the next admission one window later). The first three fail under
    admission-time accounting. A short drill reruns on the final head.
  - **Scope: one host.** The limiter covers every varswap process sharing one lock
    directory on one machine (process independence, not host independence). Running
    varswap keepers on a second host is out of this milestone; it would first need `R`
    split into per host quotas whose sum is at most `R`.
  - **429**: any process that receives one stops all varswap requests on that key for
    61 s, through the same file (a shared "blocked until" time), not only its own.
  - **Concurrent control with real HTTP**: an integration test runs a local mock Hermes
    (`std::net::TcpListener`, no new dependency) that counts requests per sliding minute
    and answers 429 above a threshold; a primary and a standby `HermesSource` in two
    processes backfill concurrently against it with a scaled clock. It asserts the total
    never exceeds `R` in any sliding window, every 404 and retry counted, and that both
    recover after a forced 429 without a second 429 inside the block.
  - The devnet drill counts the real requests from both keepers' logs per minute and
    checks the same bound.
- Drill on devnet: a new 24 bucket hourly window with primary, standby and watcher; stop the
  primary by PID for 3 hours. The watcher alerts within one interval plus grace, the standby
  fills every bucket, the window finalizes, and no encoded VAA is left open.
- *Done when* the mock control passes, the drill window finalizes complete with the alert
  times and the per minute request counts recorded in `docs/devnet.md`, and no minute
  exceeds `R`.
- **Amendment (2026-11-02, coordinator bus #44829):** the drill runs scaled: a window of 12
  intervals of 900 s (3 h), the primary stopped by PID for 1 h with `SIGSTOP` (a hung process
  that keeps its payer lock; it does not test a host failure, since primary, standby and
  watcher share one machine), standby lag `S = 600` (the validated minimum; `S <= interval −
  300` needs intervals of at least 900 s). The assertions
  are the same (alert time, the standby filling the gap, the window finalizing, no open
  encoded VAA, requests per minute within `R`), in about 3.5 h instead of about 25 h. The full
  24 hour hourly drill stays as an optional pre-mainnet item in `docs/mainnet-readiness.md`.

### I7. Reproducible publication of realized variance windows

Not a continuous index: one record per finalized or degraded window.

The account stores bucket prices, not which print filled which run, so the quality label
rests on the sample events, and the events need provenance (R1 P2).

- `varswap-admin export-window <window>` writes a bundle:
  - the account bytes at a stated slot (finalized commitment), the program id and the
    window address;
  - every transaction that touched the window, from `getSignaturesForAddress` paged back to
    its `create_window` transaction, at finalized commitment: signature, slot, block time,
    success, and for each successful one the `sample` instruction's event
    (`varswap:sample first last publish_time price expo`) with its instruction position;
  - derived fields: coverage (filled over buckets), the runs, `sum_sq`, the realized
    variance and the label.
- Reconciliation, done by export and again by verify: the successful `sample` events of
  the window must partition its filled buckets exactly (each filled bucket in exactly one
  event run, no run over an empty bucket, no overlap; the program rejects a refill, so this
  holds for a complete history), and each run's price and exponent must equal the stored
  bucket prices. History that does not reach `create_window`, a failed lookup, a gap or an
  inconsistency gives the label `unknown`: the realized variance is still reported (it
  depends only on the account), the quality is not asserted.
- Labels: `complete` (every run is one bucket), `compressed` (some run spans several buckets:
  an outage compressed into one return; count and longest run reported), `degraded` (no
  realized variance; coverage shown), `unknown` (history unproven).
- `varswap-admin verify-window <bundle>` has two modes, and says which one ran:
  - offline: checks the bundle against itself (account bytes decode to the stated prices,
    `sum_sq` and variance recomputed with `varswap-core`, events reconcile, the label is the
    one the events give). The guarantee is consistency only: the supplied snapshot and
    events agree with each other and with the program's rules. It authenticates nothing
    against the chain, and a consistent forgery (for example an edited `publish_time` that
    keeps the same run and prices) passes it;
  - `--rpc`: also refetches every listed signature and the account at finalized commitment
    and checks they exist, succeeded, invoked this program on this window and carry the
    same events, and that no successful sample transaction of the window is missing from
    the list.
- `docs/windows/`: one bundle per window plus a README table generated by the CLI, starting
  with the demo window and `7s5K`. A CI step runs the offline verify on the committed
  bundles.
- Negative controls, each must fail verify in the stated mode: offline, an edited bucket
  price in the account bytes, a deleted event (a filled bucket left uncovered), an edited
  event whose run bounds or price no longer match the buckets, and a forged label
  (`complete` on a compressed window); with `--rpc`, an edited `publish_time` that keeps the
  run and prices, an event copied from another window, and a signature removed from the
  list.
- *Done when* both devnet windows are published with a proven label, verify passes on each
  offline and with `--rpc` against devnet, and every negative control fails for its stated
  reason.

### I8. Settlement commands in `varswap-admin` (added by the coordinator, bus #45339)

The I4 drills found that `docs/runbooks/degraded-window.md` needed hand-built SDK calls.
`varswap-admin settle-swap`, `claim-long`, `claim-short` and `close-swap` read the swap, default
each token account to the associated token account of the right owner, and check it (token
program, mint, owner) before signing. Tests in LiteSVM: a degraded swap settled, claimed and
closed through the commands, and a forged-account negative per command (a non-swap or
foreign-owned swap for `settle-swap`; a destination of the other side, of another mint or not a
token account for the claims; another owner's account for `close-swap`), each also rejected by
the program when the CLI check is bypassed. A devnet run with a real swap (2 devnet USDC from
metalbook). *Done when* the tests pass and the devnet run settles, claims and closes a swap.

## Coordinator decisions (2026-11-02, bus #43363)

1. `libfuzzer-sys` and `arbitrary` approved for `fuzz/`, outside the workspace, nightly in
   CI only (I3).
2. No second Hermes key. The standby shares the key and queries only when the primary lags;
   the pacing of both keepers together stays within the per key budget (about 10 requests
   per minute on the timestamp endpoint, shared with metalbook) (I6). Allocation (bus
   #43726): varswap 3 per minute across all its processes, metalbook 7.
5. Standing rule (bus #43629): merge when codex has no remaining findings and CI is green;
   report at milestone completion and releases.
6. No branch protection on varswap's `main` (bus #43726: required CI is for optfix only).
   CI runs on every PR, and merging only on green CI is a process rule. The protection set
   on 2026-11-02 was removed the same day.
3. The Squads threshold and members stay open in `docs/governance.md`: the user decides
   before mainnet (I2).
4. The devnet upgrade authority change and the verifiable upgrade are approved, on
   conditions: the new authority's pubkey and keypair path go to the coordinator (for
   IA-VAULT), the keypair is backed up, and the old authority is kept until the transfer is
   confirmed on chain (I2).
