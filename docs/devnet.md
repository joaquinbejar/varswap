# Devnet deployment

varswap runs on Solana devnet at the addresses below, deployed on 2026-10-30 from `main` at
`aef8e3a`. Live: the program, `Config`, the SOL/USD `FeedConfig`, a demo window sampled to
the end and finalized by `varswap-keeper` (real Hermes prints through Wormhole and the Pyth
receiver), and the 2026-10-31 hourly window, which a detached keeper fills hour by hour.

## Addresses

| What | Address |
| --- | --- |
| Program | `BWqsaXvASrbErApP1ovz7vSvBLYqXGaMaZANEYf1xrB9` |
| ProgramData | `E5adgeUSG4tqpWMCU5y49kVskY6XEEfYMkh88e8GdF2g` |
| Upgrade authority (since 2026-11-02, [below](#program-build-and-authority)) | `Ai4e2jSFMV5vqLDGcyxQLf6GRbjeE5m8BiEMuqWiR9hX` |
| Config admin and deployer (upgrade authority until 2026-11-02) | `77ieYLKspRnGzAt1AK6ZVsttnTY6T7EY2k7CpTZeCPPV` |
| `Config` (`["config"]`) | `H2H2gn4mwkWoJ8ftQf97hpKJuunRzhuVPn84ZzW58uLB` |
| Collateral mint (Circle devnet USDC, SPL Token, 6 decimals) | `4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU` |
| SOL/USD `FeedConfig` (`["feed", feed_id]`) | `85wokHgbLDTDqNeWhgBgD3hu2ZBM3r8qc1UnCUFixmEF` |
| Pyth receiver (pinned in the feed config) | `rec5EKMGg6MxZYaMdyBfgwp4d5rB9T1VQH5pJv5LtFJ` |
| Window (SOL/USD, 2026-10-31 hourly) | `7s5KJ79XRwBzeL8t42ut6RkQ1yLhrRMg8z5dHQtNXTeA` |

The program id keypair was generated with `solana-keygen` and stays on the deployer's
machine; it is not in the repository. The program is upgradeable (loader v3), which
`init_config` requires: it reads the upgrade authority from `ProgramData`.

## Program build and authority

Since 2026-11-02 devnet runs the verifiable build ([`docs/verifiable.md`](verifiable.md)), and
the upgrade authority is a dedicated key instead of the deployer ([`docs/governance.md`](governance.md)).

| What | Value |
| --- | --- |
| Deployed hash (`solana-verify get-program-hash`) | `b9336e631b92f0213652a67440376fb9b275f6a5101c2c6ef809d555273386bb` |
| Built from | `main` at `06c882c` with `make verifiable` (program source unchanged since `aef8e3a`); equals that commit's build hash |
| Upgrade (`solana program deploy`, signed by `77ie…`; the CLI extended the program data to 101,064 bytes) | `3uhvEXQGH71ZTCXijv3FgcHx1G9E4bHbP4Ny3og6aUvJFt1MB4xnYnk6gUFsCcn19qhFZ3z4zVf9MWFd5iZxEiCE`, slot 517796296 |
| Upgrade authority transfer `77ie…` to `Ai4e…` (signed by both) | `4BjAJX9h6va6EKSnHo7gZRsgK9upK37GRasBg1N4spcV62V7kAcjcVfVqfLF5osu3bkitxu73AjUomXHw3wxgrL8`, slot 517796470, finalized before anything else used the new key |
| Before the upgrade | the `make program` build of `aef8e3a` (below), program hash `45513b34ccdf3edbd5c9863d72726dacc9105870c3f54c0b7ab660ed303e95a2` |

The deployed hash may lag `main`: it changes only with an upgrade, recorded here.

Checks after the upgrade (2026-11-02):

- The 24 print replay (`program/tests/replay.rs`) and all 81 program tests pass with the
  verifiable `.so`, and `program/tests/cu.rs` measures every `docs/benchmarks.md` row with
  the same CU.
- A keeper run on devnet: `varswap-keeper --watch 30` (build of `06c882c`, payer
  `5MTVStedYVor97UjAYU4shkp6RaTiKXEFupfHJN79D1`) sampled a fresh SOL/USD window of ten
  60-second intervals and finalized it.

| What | Value |
| --- | --- |
| Window | `DymB1ecC8gfgy2DD6SQgNgB1yfEpm4X7Y9MFYB6tqqGE` (`start = 1793608680`, 2026-11-02 08:38:00Z, `end = 1793609280`, `interval = 60`, 11 buckets) |
| `create_window` | `3EHDoZGguE4CFNjqy6mRjWFGhRNRVqkLVRueEBQuXCCppV23bV7RhXz89ZGnrUSzoJ3Amy7VCJtHDzdzFFErHNDn` |
| Samples | 11 posts, three transactions each, no error, no Hermes 429 |
| Final state | `Finalized`, 11 of 11 buckets, annualized realized variance `0.610348143891` (`show-window`) |
| `finalize` | `5ssHdYGDTGWBs2HjHaoArCC4mX28cTjoAYShZMZb62QBVAAPDYiCaA6bpjTZ8nhL56ZZr4A6h4NnNF2JE4oh3otk`, slot 517799874 |
| Left open | none: the keeper's scan reports no open encoded VAA |

## Build (2026-10-30 deployment)

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

## Live sampling: the demo window

`varswap-keeper` (built from PR #22) sampled a SOL/USD window of ten 60-second intervals,
2026-10-30 12:37:00Z to 12:47:00Z, with `--watch 30`. Each canonical print was fetched from
Hermes, then posted in three transactions: the encoded VAA was created and written; it was
verified under guardian set 1; then `post_update`, `sample`, `reclaim_rent` and
`close_encoded_vaa` ran in one transaction. When the last bucket filled, the keeper
finalized the window.

| What | Value |
| --- | --- |
| Window | `9P3vhP8pe8WcebFXZ77njPHddnCtLLFGFHLiAucQ2Wz8` (`start = 1793363820`, `end = 1793364420`, `interval = 60`, 11 buckets) |
| `create_window` | `5VsQaAF6gfGT7VuSsQvK7uZTubYySQKjH9fUNHiNeGh9V4yXWqe1fpxRFT9wxyjKvsvKmFTHmwyAP3UKCyET4oXG` |
| Final state | `Finalized`, 11 of 11 buckets, annualized realized variance `0.196050085777` (`show-window`) |
| `finalize` | `4HoUsuSYMEvY4ydVsapcciXT2M9rXEk8e4mHRu45WnYg3EdiNUotMHnGhs3zCUodGmUiq4dNFjsM2F1idcrhXU3t` |
| Left open | none: no encoded VAA or `PriceUpdateV2` written by the payer remains (`getProgramAccounts` on Wormhole and on the receiver) |

Posts, one row per bucket:

| Bucket | Create + init + write | Verify | `post_update` + `sample` + closes |
| ---: | --- | --- | --- |
| 0 | `i4FsAEjzaArm72T6rDYJUEfVqdQ79hfKbLBETDctfmrxJpP5yd49b6wuWeZ2bik53MYJzqubN23gBYmQNM59MGK` | `T48RXNi5uQVBFomTVBofDWEt1o5EmRwWnVbfP8hgLDNMBRjDcaZdyHj85hhpUpFFJ1ThWXCrQiWpHdEnvXJDNMN` | `3QaW1FW6EkTR3AZmeqwnFmbXLH8eBTNuRrBmh3oEKBSaXjFPqcDjv6BwE8QutoL4rzaJQM1fNNzSUvnyX2g6pcNF` |
| 1 | `2kghi4SqDGYYFdNEPhfy1nZ8uLjSVhzvCQTCLFgn3WRwiWmtoSf2qEfRKmoSqDsYxzujiqBpHxZXNNfrvQyHwbj6` | `5S1m7bSjdFAXX6pvMDowaGoUFBLzxYFNKPcNk9ZT6yHdvQyi7dA52aHvYXoKw9a2FygVckqPvwkKqJrFmAbnEdaY` | `2pkcwnJMEC9RVa9VaPPSo3d8PwudMUH1mV7wpWJyZxH7TGcXA64J46yMkBDtwzebXfujFe7w1nVuZDvKmVQPXz3b` |
| 2 | `2d88iaLn21SzerPirpch3MhVNbR2DK4xU2eC8qDVR9HMs9SvfoPjqcZAfqHY7mRosYtUf1Mbc3rrLQRqYXCuoHGF` | `2yxqQyp3rhwbphLn75e1uFX6VjkR2maPEd6GNL75r6jYT4H8pYLW9ZvPZas4aDBonnQtd9CjvgsgK1zVtA1Fv2EG` | `h3f8qkkdbJthPUPHHqNEoxPuMAaFsV56L6X9jnpBVBe5VXRWY92PtM7vc4pEcy9obpdZUFwXpAc1sbpdtLU1hK6` |
| 3 | `6AC6diTGMs2pFtvH3vyxHyFnsa25oNBG8wcDNe6WKVEhL41otC1bggdNCKcVbgUa84XY31CMuqdefTamf5m3n3x` | `2v9WMyCkSi5B2CPyCkJoXkqoFrmNkJNdtgaaNKG7Uyh4X8yhKRU9wZTrNsW7erS2mKodqzBuTqkPx2kjFiSC2Tav` | `5mmJioL7A2pTXu2vyzsVYcNaS5v4sdumuGHHaXnxrVXgBMYd2R55Yi8ALDYuNYpYZKj3UniSyAKNsFTF2JNDKvcv` |
| 4 | `9mAXj1VoyYwqezfPjbWTvPCuDC5rxssvAe3rG7oDJBfiqw61q8tU6x1vF881BnhFF22rVFQyk5G8XVJcQ8gaBBX` | `SqXXdXGFd6XyPw6H7mhXw7pcRV468VdYLH9DLs9m9jQDguYJxkygvaHpEPYh1gwG6BihG7PRom7UYMkCQKhLfD4` | `4Ao1P4Ubx9E8MsqGJkDg4pK5Z6k89rPjN1Vdx3eFVUyTjEvhHMJPgs28a2XXw74afpgbEqMLutYpckkQZbSpGb96` |
| 5 | `3ikADGubYgMuJGAzvJPDks9JvuoDKSGEtw3pNf21S3a32y1XgtrG7BLAqaPGjsV2ukPTtxT1JGPuGabyPEEtFpxz` | `32a38AQEmQ8BDKhDusa7voX3PNWAmThVGhrEAjXe2QWyn2XjCVs2oe2UAkWFAtRVFjp7GVa14Lh9mUEFuYnspMki` | `2MqQQxH2yUKpywtq3cQFjSv2g5cSzciSToaqpR9QFwkrqB4tyZrbMqz1bZuZkhY2dhRqLdLph4hfNqpyeaLhQFL6` |
| 6 | `38KSfnDixS8EEnXvoFCTbshe8ugpyqb6Ga5MGeRMp7TxrMLcc5m9AdcnAfRtgXXQ2238N4ySzkUTZf6M3hY8DU3N` | `2Jv7bkYFZnbTvMwm62PvMhWLaQATSHi2VdebjezLNmYjLAsfDoWqJKJuu2TAmryTVBrfcAo3BRa39haPvVQWg4Hr` | `3g6i9E6gCCdFM9mNGQ1zpCfxygQJQrnLWA2ZeM9NmVGqFgUPkVqedzqrrdhTYsMFeRPXWz8XAbkpmtbQsMbMBo39` |
| 7 | `6513i1aVhyvnL2HPuRxErK7JuEkdNgkhD8ud45qgwJw22pdJXN2tEb9fivWpsi8VqgQ8b7A3iMLTaGP5bsSVrAQ7` | `MskvUJJQ2K4mdDc6xzd9etgjCKnUPLkFvU1gh849i5oBFmKvrbmbDZwkJMt6TTB23QKFvar3Nfy3P355MitTRHZ` | `5FHvTJjLeHngXBvCX5iBrCrQPUd9kRt2B1PoWNjzySA4rRQvG5swv2VBojfM6YuNd8CRNgpwC3TXoSaLZeMfbjao` |
| 8 | `4vtYTRsdPGwLGZw2SJhZpkkM1hFEiZqdKostWPsMVqwiFZFG1mA9SLC14VHjj8xrotoWZHSbbpLoxw8nA5iY4f6A` | `5kNcoJd2ZhvPucYCV8XZjoMZPWp8u1uzELWJwMNmEM7ZMb1uLTZvajwgNjd4kf5TkvUYfHJ4Q7w8jBbkVW3pWJat` | `2WVZuwapJnRqMRyq9EyHoKtxCfAUTRuMn99H9tt7HWteqdEL8epX7YazSdc5UQn9p25fYLzVV29UsVVzmbMqanRU` |
| 9 | `5EUeY9b7wVZ9pfb651Zgjr7gVgNGDZVLsL9Cyy5FXzoz1psgX7B4JaccsKbxQvBdMGmgAkWqhfiXSL7wUdNuTXNT` | `3p7ANhaGL3HU5Res77h6NJwkXnkK32wjw9XsA5jKrjeC2YnGeh7S3diRi4wVrt1BAPvFhDZ26aUBfnj86fUsUb11` | `3tE4bftNbY6hswsZP2rzrBSyDFgmnzb3Nz7urz3zgofFct5EgjyR1h3hYwVZEhrCBYuj7xZNYP4NhgihjpHmmF42` |
| 10 | `5At7Wyb1NFTYBfaSAKT59EKrwVrVA9baAsCbwbn4FPWPdNE82qKd6VAbqkkwWH1dMLdAHm4XLPbjt9bx35s8btXN` | `51r9fEf9NYCjdoDevDoS4YWEX21Z1jLZdR2Vz5zyZHdKR16RmkCbWbR8CS5PYkSx8X4hR2xEEZcwVqDSdFockmp9` | `4AwLNk1F4rrcL1HB3i4iqXxC5tm8FtCJEfoyNoeR3c344HFWaNUC5pRgKg2DjcUPn5WiHb8C6DSkxTnE2Ubzz8K7` |

## The 2026-10-31 window

A detached keeper fills window `7s5KJ79XRwBzeL8t42ut6RkQ1yLhrRMg8z5dHQtNXTeA` hour by hour
and finalizes it after 2026-11-01T00:00:00Z. Its backfill deadline is 2026-11-02T00:00:00Z.

| What | Value |
| --- | --- |
| Process | LaunchAgent `com.varswap.keeper.sol-2026-10-31` on the Studio (`scripts/keeper-agent.sh`), `varswap-keeper --watch 300`, built from `main` at `68afc8d` (pyth-post 0.1.1); restarted after a crash (at most every 5 minutes) and loaded again at every login |
| Payer | `5MTVStedYVor97UjAYU4shkp6RaTiKXEFupfHJN79D1` (`~/.config/solana/varswap-devnet-keeper.json`), the keeper's own: funded with 1 SOL from the deployer `77ie…CPPV` ([transfer](https://explorer.solana.com/tx/3SeTS3YEuqWZrbABTyEQvQ6Q5estvHfvRDVZLUmTUkG94yHfdkzLZmupMprm68qRreJtjDFxozrT2moktyz9LjCC?cluster=devnet)), shared with no other poster, and locked to this process (`~/.local/state/pyth-post/payer-5MTV….lock`) |
| History | PID `96116` (from 2026-10-30, payer `77ie…`) posted buckets 0 to 8 until the Studio rebooted at 2026-10-31T08:24Z; PID `1845` (09:03Z) posted bucket 9; the agent took over at 09:16Z and was reinstalled with the `68afc8d` build and the keeper's own payer at 09:31Z |
| Log | `~/.local/share/varswap-keeper/sol-2026-10-31/keeper.log` (earlier runs: `/Users/joaquin/tmp/worktrees/varswap/keeper-run/7s5K.log`) |
| Signatures | `~/.local/share/varswap-keeper/sol-2026-10-31/keeper.sigs` (earlier runs: `keeper-run/7s5K.sigs`) |
| Status | `scripts/keeper-agent.sh status sol-2026-10-31` |
| Stop | `scripts/keeper-agent.sh uninstall sol-2026-10-31` (never `pkill`: other repos' keepers run on the Studio) |

It reads `SOLANA_URL`, `VARSWAP_PROGRAM_ID`, `VARSWAP_KEYPAIR` and `VARSWAP_HERMES_API_KEY`
from the environment; the key is never printed. It scans the payer's encoded VAAs at start
and every 12 rounds, and closes any it finds before the next post. Every Hermes answer is cached. After a 429 the
keeper waits at least 61 s, because the key's limit (about 10 requests a minute on the
timestamp endpoint) is shared with metalbook.

### Result

The window finalized on 2026-11-01T00:03:39Z, 3 minutes 39 s after its end, with every bucket
filled on time: no gap, no backfill.

| What | Value |
| --- | --- |
| Window | `7s5KJ79XRwBzeL8t42ut6RkQ1yLhrRMg8z5dHQtNXTeA` (`start = 1793404800`, `end = 1793491200`, `interval = 3600`, 25 buckets) |
| Final state | `finalized`, 25 of 25 buckets, `missing = []` (`varswap-admin show-window`) |
| Annualized realized variance | `0.068149838107` (realized volatility about 26.1%) |
| `finalize` | [`3QfLstG15JfgiJaiE7RB9LHuZCcbegEgLsKkQ24k4MdM4Q1L3NxAftaLuNrCfgQHEQnwHMP7i71DBbynLTfHcb1B`](https://explorer.solana.com/tx/3QfLstG15JfgiJaiE7RB9LHuZCcbegEgLsKkQ24k4MdM4Q1L3NxAftaLuNrCfgQHEQnwHMP7i71DBbynLTfHcb1B?cluster=devnet), paid by `5MTV…N79D1` |
| Keeper runs | PID `96116` (build `ed92a42`, payer `77ie…CPPV`) posted buckets 0 to 8; the Studio rebooted at 2026-10-31T08:24Z; PID `1845` (09:03Z) posted bucket 9; the agent (09:16Z), reinstalled at 09:31Z with build `68afc8d` and the keeper's own payer `5MTV…N79D1`, posted buckets 10 to 24 and sent `finalize` |
| Transactions | 76 (30 paid by `77ie…CPPV`, 46 by `5MTV…N79D1`), all listed in the two `.sigs` files above |
| Fees | 0.00063 SOL in total |
| Agent | uninstalled on 2026-11-01 (`scripts/keeper-agent.sh uninstall sol-2026-10-31`); `status` reports it not loaded, and it is absent from both the `gui` and `user` launchd domains |

## Run a keeper

To keep one running across crashes and reboots (macOS), install it as a LaunchAgent:

```sh
export SOLANA_URL=https://api.devnet.solana.com
export VARSWAP_PROGRAM_ID=BWqsaXvASrbErApP1ovz7vSvBLYqXGaMaZANEYf1xrB9
export VARSWAP_KEYPAIR=~/.config/solana/id.json
cargo build --release -p varswap-keeper
scripts/keeper-agent.sh install <name> <window> [<window> ...]
```

The agent runs `zsh -c 'source ~/.zshrc; exec varswap-keeper --watch 300 ...'`, so the Hermes
key comes from `~/.zshrc` and is never written into the plist. It is restarted after an error
exit, a crash or a kill, it is not restarted after the clean exit that comes once every window
is final, and it is loaded again at every login. Tested on the Studio: the agent ran on a
finalized window, exited 0 and was not restarted; after `kill -9` it was restarted. Run one
agent per payer keypair.

By hand, in the foreground:

```sh
export SOLANA_URL=https://api.devnet.solana.com
export VARSWAP_PROGRAM_ID=BWqsaXvASrbErApP1ovz7vSvBLYqXGaMaZANEYf1xrB9
export VARSWAP_KEYPAIR=~/.config/solana/id.json   # pays fees only
export VARSWAP_HERMES_API_KEY=...                  # environment only
cargo run -p varswap-keeper --release --bin varswap-keeper -- --watch 30 <window>
```

## Settlement through `varswap-admin` (2026-11-02)

The first swap on devnet, settled, claimed and closed with the I8 commands
(`docs/PLAN-improvements.md`), from `main` at `eb9fb5e` plus the I8 branch. Collateral: Circle
devnet USDC, 2 units sent by metalbook (`5WbkRhbvzdJSKvf165Kfi5qVyoj1PPkVT1cTqKgHUVVA7go362EX1Q9ntBBfTtsh2pjEskzGQpwGtaQroxz6E7Zw`), one passed to the short
(`4MaFtHK1dhRkf138WAAgSkPLmCPq7CFEiaDNiWGWdte32MzdAiEcnB7iDoMn5YVHEEdsJN4ZZ9Ypm6oNhRvHXPEj`).

| What | Value |
| --- | --- |
| Window | `9Pm5ttAfxvsWyrbQ7GnYQ7mo1SkJZLCve66A3diPtGLA` (10:52 to 10:54Z, two 60 s intervals), `create_window` `3KYxX9P6KL5BcAL1kHUgTw2t94p81vzrPVGG5dzWHu3nXrbbfxgYt5tppQFitVhSB1DP96rCyjV9owuy3xjknnXp`; sampled by `varswap-keeper` and finalized, realized variance `0.058317865661` |
| Long, short, payer | `77ieYLKspRnGzAt1AK6ZVsttnTY6T7EY2k7CpTZeCPPV`, `CmNdYPuAxpLKyfEjRpDk6Pvvqk42SqTkZWv6qwX1dnnk`, `77ie…` |
| Terms | notional 1,000,000 atoms (1 USDC per unit of variance), strike 0.01, cap 0.0625: collateral 10,000 (long) and 52,501 (short) atoms |
| `open_swap` (`cli/examples/open_swap.rs`) | swap `AquGkKfwgjvXqWG3yae121AM866rY2amBGiUi4hGkU7o`, `3HqcqvJKf5XMxqPgQZG7WBZFdKgve8S1RNWyPDmoywe4zHsBoQXgWy2Y9gDexRjpHrts5RaqCmn64h6P6uSm71At` |
| `varswap-admin settle-swap` | `2LB5LoPzaEVXZ2gDfvYpgg8Zcg79CD5pdqm8zonJLJGMEn5nvZYH5Hr7owxiPJu8AxsNJFgsP19w3sRqMG1tcY2J`: payouts 58,317 (long) and 4,184 (short), sum 62,501 = the collateral |
| `varswap-admin claim-long` | `gUGqVRscgb6dTkzazxnWeBJGWWS3xwGBVQMUh9VaWCzxG9NqEjXZ1ePaJwp2vJnNKvzYypvTMs6dErRPJpeZkzd` |
| `varswap-admin claim-short` | `3Gdx5Yoj3ZwWNAWN8v2P3P9e5At8EMQkXUfnLqQSS3oskDCF1W8QcLf891x7nkvDq3yZpqf9AW7Ck64ZQKQDQfzA` |
| `varswap-admin close-swap` | `V7iqxZ1fVhMmdXhAbvyircEx1TMe5P8L24NaXyxryAPXBTR8N2sWGRgVtb7FenEH2UMroA9vdu3xQS3Dn3tmVRM`; the swap account no longer exists |
| Balances after | long 1.048317 USDC (1 − 0.010000 + 0.058317), short 0.951683 USDC (1 − 0.052501 + 0.004184) |

## Standby and watcher drills (I6, 2026-11-02)

The scaled I6 drill (`docs/PLAN-improvements.md`): a primary keeper, a standby keeper
(`--standby-lag 600`, its own payer `AaXbMDDnJFghzgaGc7wi3v5MsLJqS2PFvp6ZucKsYxqU`) and
`varswap-watch` (`--grace 600`, alerts appended to a file), all three LaunchAgents on the
Studio, on one 15-minute-interval window each. The primary (payer
`5MTVStedYVor97UjAYU4shkp6RaTiKXEFupfHJN79D1`) is stopped with `kill -STOP <pid>` (a hung
process: alive, holding its payer lock) and resumed with `kill -CONT <pid>`. Every varswap
Hermes request is logged as `hermes: request at <ms> ms, HTTP <status>`; the counts below are
over the primary's and the standby's logs together. All three ran on one host, so this shows
process independence only; host independence is a pre-mainnet gate
(`docs/mainnet-readiness.md`).

### Run 1: 2b6beda, admission-time limiter

Binaries built from `2b6beda` (keeper
`e992e36211ac68e3b7d48333749ff156f92cee30333d4b991d43672d6b299747`, watch
`0eafbcf26edf65c30d0cbdf80e9b49dc28f6743874df2f3d9e10956dcbf42e32`), before the review of
#39 moved the limiter to the wire-time contract; this run does not certify that limiter (run
2 does). Window `5Tz9Z2BdUCo4B95KqYefNRPDrQxf5e27woJMFQCdHxFb`, 13 buckets of 900 s from
10:30Z to 13:30Z.

| Time (UTC) | Event |
| --- | --- |
| 10:31:35, 10:46:40 | primary posts buckets 0 and 1 |
| 11:00:08 | `kill -STOP 56034` (primary) |
| 11:10:30 | watcher: `lag`, bucket 2 empty for 629 s |
| 11:11:38 | standby posts bucket 2 (`4Qtte3GCkyoYT7mZUgw1D9BbXetejHaqVdxoraJRFQKymAxetKQvqf6QoVNxWodtH5aJsKSujiFayyXtPPtRh7qx`); `resolved lag` at the 11:12:30 pass |
| 11:25:45 to 11:57:07 | the same for buckets 3, 4 and 5: `lag` at 644, 654 and 605 s, standby posts at 11:26:44, 11:41:49 and 11:56:54, each resolved at the next pass |
| 12:00:03 | `kill -CONT 56034`; the primary posts bucket 6 at 12:00:07 and every later one |
| 13:30:38 | primary posts bucket 12 and finalizes: `5voTsAL6QvK3jfwChkpFBbrMswgxcoghGPWoUFofGGMNvKRcYXLWVz5JS2eJ3z4ARBmXDMTV8HjwWZ1gsfDxZAUy`, slot 517870546 |

Result: finalized, 13 of 13 buckets, 15 transactions on the window (create, 13 posts,
finalize), none failed; 13 Hermes requests, all HTTP 200, at most 1 in any 60 s; no encoded
VAA left open by either payer.

### Run 2: 0bf234f, wire-time limiter

The short rerun on the final code of #39. Binaries built from `0bf234f` (keeper
`a340b9eb2558fa5dc64ff243a72619c8404257de098d0af94aa0e418bb8c60e2`, watch
`31e955a36804287cd3ddd47cb398ad2c59073845323eeaedcfabc386a66693dd`), copied into each
LaunchAgent's directory; the primary and the standby run the same keeper binary. Window
`5GAX5tC1rVdU1M5Us8ZtEramaUAHXGJ6CEDjBYafzqj3` (created by
`2XaVKAn8qMqaAW3C2GtwkkpYtQF8Am1LE7gBKcEfYZN2gYUQPEuyyzYD4TbRLUVXMkaR5d72R2AXXBzSzN8L6tHZ`),
5 buckets of 900 s from 13:45Z to 14:45Z. The watcher also ran the `hermes-hung` check
(`--hermes-rate 3`, `--hermes-hung-after 120`) on every pass.

| Time (UTC) | Event |
| --- | --- |
| 13:47:10, 14:02:15 | primary posts buckets 0 and 1 |
| 14:05:04 | `kill -STOP 26619` (primary) |
| 14:25:46 | watcher: `lag`, bucket 2 empty for 644 s |
| 14:27:13 | standby posts bucket 2 (`2aTwhWQtu88oyXkLH1PzwnEDqwr7jVCtg1beBbmswbrSDSetaSLEFHLs1yPLffEg4qMskb1foyuYAAKUbRJxLMYd`); `resolved lag` at the 14:27:46 pass |
| 14:35:06 | `kill -CONT 26619`; the primary posts bucket 3 at 14:35:10, before the standby's 600 s lag or the watcher's grace was reached |
| 14:45:16 | primary posts bucket 4 at 14:45:14 and finalizes: `3hvAqH37fN9tmABjoA6oZhtfgahPpp8A3m8Gh3Q29Vp1xocCEExvkX45PyDyzVBLTPzwTuVi8SBLCjXQpCdmuLyJ`, slot 517889248 |

Result: finalized, 5 of 5 buckets, 7 transactions on the window (create, 5 posts, finalize),
none failed; 5 Hermes requests, all HTTP 200, at most 1 in any 60 s; no encoded VAA left open
by either payer. The watcher's `hermes` line showed `held 0` on all 59 passes and
`hermes-hung` never fired: the SIGSTOP landed between two requests, so the stopped primary
held no Hermes slot. A process stopped while it holds a slot, and the alert when all R are
held, are covered with real processes by `test_stopped_then_killed_process`
(`keeper/tests/limiter.rs`).

## Daily hourly series (from 2026-11-02)

Hourly sampling runs continuously on devnet as a series of daily windows on the SOL/USD feed:
window k starts at day k's 17:00Z and ends 24 h later, 25 buckets of 3600 s. A window ends,
so the series needs a new one every day. `scripts/window-roll.sh run` creates the window that
starts at the next 17:00Z if `show-window` reports it not found. It then reinstalls the keeper
and watcher agents with the series' open windows, but only when one of them is missing from
the agent's plist. The window address is derived from the spec, so every run is idempotent.
If any read fails, the run stops before it sends anything. `scripts/window-roll.sh install
<name>` runs it under launchd at load and every 6 hours.

| Agent | Configuration |
| --- | --- |
| `com.varswap.keeper.devnet-hourly` | payer `5MTVStedYVor97UjAYU4shkp6RaTiKXEFupfHJN79D1`, `--watch 300` |
| `com.varswap.watch.devnet-watch` | `--payer 5MTV… --grace 600 --min-balance 300000000` (0.3 SOL; the devnet airdrop script refills the payer every 8 h), alerts appended to `~/.local/share/varswap-keeper/devnet-watch/alerts.log` |

| Window | Start | Created by |
| --- | --- | --- |
| `7ywXkV5Sa3fGHKhTAcdXqR5jkNg5J7pqvGQg1Hh6XjG1` | 2026-11-02 17:00Z | `3UuXz2DFxSpCYPeLmxDCWSdi4Qafnko8MiFi9GPH899KwjgCVZf9AqENDKKzNegyzK23zbzMBfxdSeE2oJcVMWNq` (by hand) |
| `CjtkCZY1BEzN8MFgrbViGtW9KEUqP6vMKu7CCPRqiATG` | 2026-11-03 17:00Z | `3eJVefeKHPb4Gk5K4QdDt7PNquK81T1VRaNwwvwcEYq8VvXrgx7SKbk8R8d9rXRvAPje8UBZVNtnj7wBjtBwjRfP` (by hand) |

The roll job is installed once its PR has been reviewed. Until then, the windows above are
created by hand. The roll job's configuration, written into its plist (no secret):

```shell
SOLANA_URL=https://api.devnet.solana.com \
VARSWAP_PROGRAM_ID=BWqsaXvASrbErApP1ovz7vSvBLYqXGaMaZANEYf1xrB9 \
VARSWAP_KEYPAIR=$HOME/.config/solana/id.json \
VARSWAP_ROLL_FEED=ef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d \
VARSWAP_ROLL_KEEPER=devnet-hourly \
VARSWAP_ROLL_KEEPER_KEYPAIR=$HOME/.config/solana/varswap-devnet-keeper.json \
VARSWAP_ROLL_WATCH=devnet-watch \
VARSWAP_ROLL_WATCH_ARGS="--payer 5MTVStedYVor97UjAYU4shkp6RaTiKXEFupfHJN79D1 --grace 600 --min-balance 300000000" \
VARSWAP_WATCH_ALERT_CMD='printf "%s %s %s %s\n" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$VARSWAP_ALERT_KIND" "$VARSWAP_ALERT_SUBJECT" "$VARSWAP_ALERT_MESSAGE" >> "$HOME/.local/share/varswap-keeper/devnet-watch/alerts.log"' \
sh scripts/window-roll.sh install devnet-roll
```

Test of the script, 2026-11-02 15:54Z, on a throwaway series anchored at 18:00Z, with test
agents and the standby payer:
- The install's dry run reported the window to create and both agents to reinstall.
- The first launchd run created `AGpn2Ux83LeRcXsEE3fbjRsEYnQ7oiyDj8v4YYqoJzst`
  (`wwNAr7WEUYtjgWTwzbR7bTd9gELAk3WbTggQEv79Bt4eQnRn37CWVV4YJbxKEhsnWkn2j7m1jGHUBQ3Tg7utNQi`)
  and installed both agents.
- A second run, by `launchctl kickstart`, found the window, left both agents running and
  changed nothing; the keeper kept its PID.
- With an unreachable RPC the run stops at the first `show-window`, sends nothing and exits
  1. Its error mentions "AccountNotFound", which is not the not-found answer the script looks
  for.

The test agents were then uninstalled; `AGpn…` stays pending and unsampled.
