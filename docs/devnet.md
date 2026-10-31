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
| Process | LaunchAgent `com.varswap.keeper.sol-2026-10-31` on the Studio (`scripts/keeper-agent.sh`), `varswap-keeper --watch 300`, built from PR #24 (`f96af44`, the 300-block leftover scan); restarted after a crash and loaded again at every login |
| History | PID `96116` (from 2026-10-30) posted buckets 0 to 8 until the Studio rebooted at 2026-10-31T08:24Z; PID `1845` (09:03Z) posted bucket 9; the agent took over at 09:16Z |
| Log | `~/.local/share/varswap-keeper/sol-2026-10-31/keeper.log` (earlier runs: `/Users/joaquin/tmp/worktrees/varswap/keeper-run/7s5K.log`) |
| Signatures | `~/.local/share/varswap-keeper/sol-2026-10-31/keeper.sigs` (earlier runs: `keeper-run/7s5K.sigs`) |
| Status | `scripts/keeper-agent.sh status sol-2026-10-31` |
| Stop | `scripts/keeper-agent.sh uninstall sol-2026-10-31` (never `pkill`: other repos' keepers run on the Studio) |

It reads `SOLANA_URL`, `VARSWAP_PROGRAM_ID`, `VARSWAP_KEYPAIR` and `VARSWAP_HERMES_API_KEY`
from the environment; the key is never printed. It scans the payer's encoded VAAs at start
and every 12 rounds, and closes any it finds before the next post. Every Hermes answer is cached. After a 429 the
keeper waits at least 61 s, because the key's limit (about 10 requests a minute on the
timestamp endpoint) is shared with metalbook.

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
