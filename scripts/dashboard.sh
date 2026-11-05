#!/bin/sh
# Generates varswap's part of the shared dashboard (docs/dashboard.md, docs/PLAN-phase2.md P3c):
# syncs the realized volatility records from devnet, computes the index, and writes the section
# and the shell.
#
#   scripts/dashboard.sh <out-dir>
#
# Writes, under <out-dir>:
#   index.html dashboard.mjs dashboard.css sections.json   the shell, copied from site/
#   varswap/section.json
#   varswap/index/sol-usd-hourly/index.json                every day, plus the generator's commit
#   varswap/index/sol-usd-hourly/sync.json
#   varswap/index/sol-usd-hourly/records/<day>-<address>.json, and records/sync.json as its state
#
# Rerun on the same <out-dir> it resumes: series-sync keeps what is there. Any failure exits non
# zero with the existing section and index untouched, so the shell shows an old section as stale
# rather than a partial one as fresh.
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
dir=$out/varswap/index/sol-usd-hourly
mkdir -p "$dir/records"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

"$admin" series-sync "$spec" --out "$dir/records" >"$work/sync-summary.json"
"$admin" vol-index "$spec" --records "$dir/records" --history --json >"$work/history.json"

node "$root/scripts/dashboard-section.mjs" \
	--history "$work/history.json" \
	--name "SOL/USD" \
	--commit "$(git -C "$root" rev-parse HEAD)" \
	--generated-at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
	--section "$out/varswap/section.json" \
	--index "$dir/index.json" \
	--records "$dir/records"

cp "$dir/records/sync.json" "$dir/sync.json"
for f in index.html dashboard.mjs dashboard.css sections.json; do
	cp "$root/site/$f" "$out/$f"
done
