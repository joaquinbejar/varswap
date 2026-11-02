# Hermes: 429 and key rotation

The API key is shared with metalbook: about 10 requests per minute on the timestamp endpoint,
of which varswap's allocation is 3 per minute across all its processes (`docs/governance.md`).
The host-wide limiter of I6 (README, "Hermes budget") enforces that allocation for every
varswap process on one host: at most `--hermes-rate` requests reach Hermes in any 60 s, and a
429 pauses them all for 61 s. It does not cover another host: running varswap keepers on a
second machine needs the allocation split into per-host quotas first, coordinated with the
coordinator.
The key's only storage is `~/.zshrc`, which exports `VARSWAP_HERMES_API_KEY`. It is never in the
repository, a plist, a log or a command line, and never printed; the keeper scrubs it from
errors.

## 429

Detect: `price source rate limit (HTTP 429)` in the keeper log. The keeper backs off at least 61 s (Hermes asks for
60 s) and retries at its next tick; answers it already has are cached and cost nothing.

- One 429 now and then: nothing to do.
- Repeated 429s: check that only one varswap keeper per window runs
  (`launchctl list | grep com.varswap.keeper`), and ask the coordinator whether metalbook is in a
  settlement window; avoid manual backfills then.
- Never restart keepers in a loop to "push through": each start queries Hermes again.

## hermes-hung

Detect: a `hermes-hung` alert from `varswap-watch`. Every slot of the host's Hermes budget is
held by a request in progress for longer than `--hermes-hung-after` (120 s; a request
completes within the HTTP client's 10 s timeout). The owners are alive but stopped or hung: a
stopped process keeps its slot for as long as it is stopped, by design (the wire-time
contract), so no varswap process on the host can reach Hermes, the standby included.

1. The alert names the owner PIDs. Check each one: `ps -o pid,stat,etime,command -p <pid>`
   (`T` in `stat`: stopped).
2. A process stopped by an operator on purpose (a drill): `kill -CONT <pid>` when the drill
   allows it.
3. Otherwise kill it by PID, never by name (other repos run keepers on the same host):
   `kill <pid>`, then `kill -9 <pid>` if it is still there after 10 s (a stopped process only
   handles `SIGTERM` once continued). Its lock is released when it exits; the next request
   from another process finds the slot free and counts it one more minute, then the budget is
   back. The LaunchAgent restarts a killed keeper.
4. The alert resolves at the next pass that finds a slot free.

## Rotate the key

1. Get the new key from the owner (the user); the coordinator records it.
2. Replace the `export VARSWAP_HERMES_API_KEY=...` line in `~/.zshrc` (edit the file; do not
   echo the key in a terminal whose history is kept).
3. Restart every varswap keeper (`kill <pid>` per agent, [keeper.md](keeper.md)): the agent
   sources `~/.zshrc` at each start. Tell metalbook: it uses the same key.
4. Verify: the next tick posts (`posted 1`), no `401`/`403` in the log.
