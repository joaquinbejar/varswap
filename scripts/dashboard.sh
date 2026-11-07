#!/bin/sh
# Generates varswap's part of the shared dashboard (docs/dashboard.md, docs/PLAN-phase2.md P3c):
# syncs the realized volatility records from devnet, computes the index, and publishes the section
# and the shell as one generation.
#
#   scripts/dashboard.sh <out-dir>
#
# Writes, under <out-dir>:
#   records/                 series-sync's state; a rerun resumes from what is here
#   generations/gen.XXXXXX/  the current generation, the one before it, and the one being built
#   current                  symlink to the published generation: serve or deploy this
#
# and a generation holds:
#   index.html dashboard.mjs dashboard.css sections.json   the shell, copied from site/
#   varswap/section.json
#   varswap/index/sol-usd-hourly/index.json                every day, plus the generator's commit
#   varswap/index/sol-usd-hourly/sync.json
#   varswap/index/sol-usd-hourly/records/<day>-<address>.json, and records/sync.json as its state
#
# A generation is built and checked apart, and goes live in one step, by renaming the symlink: a
# reader of `current` gets a whole generation, the new one or the old one. Any failure before that
# rename exits non zero with `current` untouched, so the shell shows an old section as stale rather
# than a partial one as fresh. Only one run at a time holds <out-dir>: a second exits non zero.
#
# It reads the chain through SOLANA_URL (default: devnet's public RPC), at series-sync's default
# rate. It holds no key, signs nothing and makes no Hermes request.
#
#   SOLANA_URL          RPC endpoint
#   VARSWAP_ADMIN_BIN   varswap-admin (default target/release/varswap-admin)

set -eu

[ $# -eq 1 ] || { echo "usage: $0 <out-dir>" >&2 && exit 2; }
root=$(cd "$(dirname "$0")/.." && pwd)
out=$1
admin=${VARSWAP_ADMIN_BIN:-$root/target/release/varswap-admin}
[ -x "$admin" ] || { echo "$admin missing: cargo build --release -p varswap-cli" >&2 && exit 1; }
SOLANA_URL=${SOLANA_URL:-https://api.devnet.solana.com}
export SOLANA_URL

spec=$root/docs/index/series/sol-usd-hourly.json
mkdir -p "$out/records" "$out/generations"

# mkdir is the one atomic test and set a shell has. A lock is never taken over, even when its pid
# is gone: two runs that took over the same dead lock at once would both publish and prune, and
# one could remove the generation the other made live. A run killed with SIGKILL, or a reboot,
# leaves the lock, and the next run fails until it is removed by hand.
lock=$out/.lock
if ! { mkdir "$lock" 2>/dev/null && echo $$ >"$lock/pid"; }; then
	holder=$(cat "$lock/pid" 2>/dev/null || true)
	if [ -n "$holder" ] && kill -0 "$holder" 2>/dev/null; then
		echo "another dashboard.sh holds $lock (pid $holder, running)" >&2
	else
		echo "$lock is held by pid ${holder:-unknown}, which is not running: if no dashboard.sh is active, remove $lock by hand" >&2
	fi
	exit 1
fi

work=
stage=
cleanup() {
	[ -z "$work" ] || rm -rf "$work"
	# The generation stays once `current` points at it, whatever failed after the rename.
	if [ -n "$stage" ] && [ "$(readlink "$out/current" 2>/dev/null || true)" != "generations/$(basename "$stage")" ]; then
		rm -rf "$stage"
	fi
	rm -rf "$lock"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

work=$(mktemp -d)
stage=$(mktemp -d "$out/generations/gen.XXXXXX")
chmod 755 "$stage"
dir=$stage/varswap/index/sol-usd-hourly
mkdir -p "$dir"

"$admin" series-sync "$spec" --out "$out/records" >"$work/sync-summary.json"
"$admin" vol-index "$spec" --records "$out/records" --history --json >"$work/history.json"

# Only the records: a run killed mid write can leave a .tmp beside them.
mkdir "$dir/records"
cp "$out"/records/*.json "$dir/records/"
cp "$dir/records/sync.json" "$dir/sync.json"
for f in index.html dashboard.mjs dashboard.css sections.json; do
	cp "$root/site/$f" "$stage/$f"
done

node "$root/scripts/dashboard-section.mjs" \
	--history "$work/history.json" \
	--name "SOL/USD" \
	--commit "$(git -C "$root" rev-parse HEAD)" \
	--generated-at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
	--section "$stage/varswap/section.json" \
	--index "$dir/index.json" \
	--records "$dir/records"

node "$root/scripts/dashboard-publish.mjs" "$out" "$stage"
