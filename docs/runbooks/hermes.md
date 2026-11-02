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

## Rotate the key

1. Get the new key from the owner (the user); the coordinator records it.
2. Replace the `export VARSWAP_HERMES_API_KEY=...` line in `~/.zshrc` (edit the file; do not
   echo the key in a terminal whose history is kept).
3. Restart every varswap keeper (`kill <pid>` per agent, [keeper.md](keeper.md)): the agent
   sources `~/.zshrc` at each start. Tell metalbook: it uses the same key.
4. Verify: the next tick posts (`posted 1`), no `401`/`403` in the log.
