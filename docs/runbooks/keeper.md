# Keeper: install, restart, stop, check

## Detect

- `scripts/keeper-agent.sh status <name>`: loaded or not, its PID and last exit status.
- `tail ~/.local/share/varswap-keeper/<name>/keeper.log`: one line per window and tick,
  `posted N, already filled N, skipped N, finalized …, done …, open encoded VAAs [...]`.
- `varswap-admin show-window <window>`: `filled`, `missing`, `state`. A bucket whose boundary
  is more than one watch interval (300 s) plus a few minutes old and still missing means the
  keeper is behind.

## Install (macOS LaunchAgent)

```sh
scripts/keeper-agent.sh install <name> <window> [<window> ...]
```

It copies `target/release/varswap-keeper` (or `VARSWAP_KEEPER_BIN`) to
`~/.local/share/varswap-keeper/<name>/` and loads `com.varswap.keeper.<name>`: `--watch 300`,
restarted after an error exit, a crash or a kill, never more than once every 300 s
(`ThrottleInterval`), not restarted after the clean exit that comes once every window is
final, loaded again at login. One agent per payer: the binary takes `PayerLock` and refuses to
start if another process holds the payer.

## Restart

A crash or a kill restarts by itself. To restart by hand (for example after a new build):

```sh
scripts/keeper-agent.sh status <name>          # note the PID
kill <pid>                                     # exact PID only; launchd starts it again
scripts/keeper-agent.sh status <name>          # a new PID within 300 s of the last start
```

To change the binary, `uninstall` and `install` again: the agent runs its own copy.

## Stop

```sh
scripts/keeper-agent.sh uninstall <name>
scripts/keeper-agent.sh status <name>          # "not loaded"
```

## Verify

The log shows ticks again after the restart; missed buckets are posted at the first tick
(backfill until the window's deadline); `open encoded VAAs []` once any leftover is closed
([orphaned-vaas.md](orphaned-vaas.md)). Drill: [drills.md](drills.md), restart.
