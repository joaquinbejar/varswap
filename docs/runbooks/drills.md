# Drills

Runbooks exercised on devnet on 2026-11-02, program `BWqsaXvASrbErApP1ovz7vSvBLYqXGaMaZANEYf1xrB9`
(the verifiable build, `docs/devnet.md`), keeper built from `main` at `6731d34`. Times are UTC.
Processes were stopped by exact PID only.

## Restart and backfill ([keeper.md](keeper.md), [backfill.md](backfill.md))

A LaunchAgent keeper (`com.varswap.keeper.drill-restart`, payer
`5MTVStedYVor97UjAYU4shkp6RaTiKXEFupfHJN79D1`) on a SOL/USD window of eight 300-second intervals,
`8vgqkuTDgQuMXB1pr3QCkR8SsQhT4jJ1TMUC8neSx1yv` (09:05 to 09:45, 9 buckets,
`create_window` `3TRtozvLYmnezAKY762Bh4QuzdDUJCFYnfHNHEtnk8WVbbmsQoQxW6ddFLj8gBNK9GKCoreRsgMiKot8Dc1HpqUM`).

| Time | Event |
| --- | --- |
| 08:59:05 | agent installed; first start |
| 09:09:11, 09:14:35 | buckets 09:05 and 09:10 sampled on time |
| 09:17:00 | `kill -9` of the keeper (PID 79291); launchd restarted it the same second (PID 74826): the last start was more than 300 s before |
| 09:17:30 | `kill -9` again (PID 74826), inside the 300 s `ThrottleInterval` |
| 09:15, 09:20 | boundaries passed with no keeper posting (09:15 fell in the 30 s run, which had not ticked yet) |
| 09:22:01 | launchd restarted it (PID 29312), 301 s after the previous start |
| 09:22:04, 09:22:27 | the first tick backfilled both missed buckets (09:15 and 09:20): `posted 2` |
| 09:27:43 | bucket 09:25 on time |
| 09:32:48, 09:37:52, 09:42:57, 09:48:01 | buckets 09:30 to 09:45 on time |
| 09:48:03 | `finalize`: `2togXG1bmKX2thx8ifEnJysmbyeyL7DdS44jS1ZsqFdWEjkg17umnKdm9f59ymzqweuwpKhLi1koJpoX7ByET1Me`; `Finalized`, 9 of 9 buckets, realized variance `0.041891463736` |
| after 09:48 | the keeper exited 0 (`LastExitStatus = 0`, 3 runs) and launchd did not restart it; the agent was then uninstalled (`status`: not loaded) |

Recovery: a kill restarts at once unless the agent started less than 300 s before, in which
case launchd waits out the `ThrottleInterval`; the first tick after a restart backfills every
missed bucket (26 s for two here).

## Orphaned encoded VAA ([orphaned-vaas.md](orphaned-vaas.md))

Killing a keeper between the transactions of a post is a race (it prints a post's signatures
together, after the post), so the drill opens the orphan deterministically:
`cargo run -p pyth-post --example open_encoded_vaa -- <payer>` creates, initializes and
writes an encoded VAA with the payer as write authority and exits, as a poster killed after the
first transaction would. Payer `77ieYLKspRnGzAt1AK6ZVsttnTY6T7EY2k7CpTZeCPPV` (not used by the
restart drill's agent).

| Time | Event |
| --- | --- |
| 09:22:22 | encoded VAA `9VHBu1ZQnoeuqMqB5HyCMhj6Jcc1e7VzWR9WMr9G1wEh` opened (rent 1,209,040 lamports), `4uhBTBxNphB5Zr3vfmDjLSF58krB2ANZC7db5FvyVRrZSk4Beayu5opg2VdNbvrU3R7PFCMkxYLoEsu2maFAEmdQ`, slot 517808397 |
| 09:22:32 | `varswap-keeper --watch 20` started on window `8G4y9wKKjZ2VVJN7iaosXVok745NpVPpoTVHpS5dgGtz` (seven 60-second intervals, 09:24 to 09:31) with the same payer; its first scan saw the account younger than 300 blocks and left it |
| about 09:27 | the scan after 12 ticks found it unchanged for 300 blocks or more: `scan: 1 leftover encoded VAA(s) queued for closing`, `open encoded VAAs [9VHBu1ZQ…]` |
| 09:28:12 | closed by the next tick: `5xzEfazG8UZQ9qDAZYDSFmG2tH5UCBacaMmd88Gdki2feA6tMnvppdeTQs3MJMday9js8KMUEpTV8vXTvkDpn4bs`, slot 517809853 (1,456 slots after it was opened) |
| 09:31:29 | the window finalized, 8 of 8 buckets; the keeper exited 0 with `open encoded VAAs []` |

`solana account 9VHBu1ZQnoeuqMqB5HyCMhj6Jcc1e7VzWR9WMr9G1wEh` then answers `AccountNotFound`:
the rent is back with the payer. The keeper kept sampling throughout; the leftover never
blocked a post.

A first attempt, killing the keeper when its fourth signature appeared, killed it after the
whole post had landed (no orphan), and a second, polling Wormhole's accounts every 0.3 s on the
public RPC, drew HTTP 429s from the RPC and was stopped: hence the example.

## Key compromise ([key-compromise.md](key-compromise.md)): dry run

Walked through without rotating any key: the payer section against the restart drill's agent
(`status`, `uninstall` and `install` are the commands the restart drill used); the upgrade
authority section against `solana program show` and `solana-verify get-program-hash` (read
only); the Hermes section against `hermes.md`. No transaction was sent.
