# Mainnet readiness

What must be true before varswap is deployed on mainnet, with an owner and the evidence for
each line (`docs/PLAN-improvements.md`, I5). Nothing here is a mainnet action, and no line is
checked off without its evidence linked. Owners: **user** (Joaquin, decisions and keys),
**coordinator** (SOLANA-PROJECTS, sequencing and cross-repo agreements), **varswap** (this
repository's agent), **auditor** (external).

Status: `open`, `in progress`, `done` (with evidence), `n/a` (with the reason).

## Addresses and external programs

| Item | Owner | Status | Evidence |
| --- | --- | --- | --- |
| Mainnet Pyth receiver program id equals `pyth_lite::RECEIVER_PROGRAM_ID` (`rec5EKMGg6MxZYaMdyBfgwp4d5rB9T1VQH5pJv5LtFJ`: the owner of the mainnet fixtures in `pyth-lite/fixtures` and of the devnet updates) and its config account and treasury layout match `pyth-post` | varswap | open | |
| Mainnet Wormhole core program id and current guardian set index, and the `pyth-post` builders against a mainnet program dump (as `pyth-post/tests/devnet_programs.rs` does for devnet) | varswap | open | |
| Push oracle and sponsored feeds: whether the target feeds (SOL/USD first) are sponsored on mainnet, their update cadence, and whether varswap still needs its own posting (the keeper posts historical canonical prints, which sponsored feeds do not provide) | varswap | open | |
| Collateral mint: mainnet USDC address, token program, decimals | user | open | |

## Keys and governance

| Item | Owner | Status | Evidence |
| --- | --- | --- | --- |
| Squads v4 multisig created; threshold and members decided | user | open | [`docs/governance.md`](governance.md) |
| Upgrade authority is the multisig vault before any other instruction; `init_config` signed by the multisig, so the config admin is the vault | user, varswap | open | devnet rehearsal: authority moved to a dedicated key (`docs/devnet.md`, transfer `4BjAJX9h…`) |
| Program keypair generated and stored offline; never committed | user | open | |
| Keeper payers: one per poster instance, small balances, top-up runbook | varswap | open | `docs/runbooks/` (I4) |
| A dedicated mainnet Hermes key with a known per-key budget (the devnet key is shared with metalbook: 3 requests per minute for varswap) | user, coordinator | open | |

## Code and verification

| Item | Owner | Status | Evidence |
| --- | --- | --- | --- |
| CI green on every pull request (`checks`, `test`, `bench`) | varswap | done | `.github/workflows/ci.yml`, PR #31 |
| Verifiable build: build hash reproduced by CI, deployed hash recorded after each upgrade | varswap | done (devnet) | [`docs/verifiable.md`](verifiable.md) (CI job `verifiable`, build hash `b9336e63…`), [`docs/devnet.md`](devnet.md#program-build-and-authority) (deployed hash equal, upgrade `3uhvEXQG…`), PR #36 |
| Fuzzing of `pyth-lite` parsing, window sampling, swap conservation and the `sample` instruction, with fault controls | varswap | in progress | `fuzz/`, PR #35 (I3) |
| Audit scope written and an audit completed, findings fixed | varswap, auditor | open | `docs/audit-scope.md` (I4) |
| Runbooks drilled on devnet | varswap | open | `docs/runbooks/` (I4) |
| Degraded policy decided and documented | varswap | done | [`docs/adr/0001-degraded-unwind.md`](adr/0001-degraded-unwind.md) |

## Operations

| Item | Owner | Status | Evidence |
| --- | --- | --- | --- |
| Primary and standby keepers with their own payers, a shared Hermes limiter, and `varswap-watch` alerts | varswap | done (devnet) | I6, PR #39: scaled devnet drill on the final build `0bf234f` (`docs/devnet.md`, "Standby and watcher drills") |
| Optional: the full drill at production cadence (a 24 bucket hourly window, the primary stopped for 3 h), as in the unamended I6 plan | varswap | open | |
| Independent-host resilience: a standby and a watcher on a machine other than the primary's (the I6 drill ran all three on one host, and the Hermes limiter is per host: `R` must first be split into per host quotas) | varswap, coordinator | open | |
| Window records published with provenance and quality labels | varswap | open | I7 |
| Monitoring: who receives alerts and how fast they act | user | open | |

## Parameters and launch limits

| Item | Owner | Status | Evidence |
| --- | --- | --- | --- |
| Per feed parameters (`tolerance`, `backfill_deadline`, `max_conf`, `min_interval`, `max_buckets`, exponent) chosen with the study's unwind and error figures at that guard ([ADR 0001](adr/0001-degraded-unwind.md)) | user, varswap | open | |
| Historical outage rate of each launch feed measured with a Hermes key, compared with the study's assumed regime | varswap | open | |
| Launch limits: maximum notional per swap and in total, and which feeds | user | open | |
| Freezing the program (`--final`) after the audit: an explicit decision | user | open | |
| Legal review of offering variance swaps | user | open | |
