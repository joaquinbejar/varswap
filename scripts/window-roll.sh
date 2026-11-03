#!/bin/sh
# Keeps a daily series of windows going on one feed (devnet): every `run` makes sure the window
# that starts at the next VARSWAP_ROLL_HOUR:00 UTC exists, and points the keeper and watcher
# LaunchAgents at the windows of the series that are not final yet.
#
#   scripts/window-roll.sh run
#   scripts/window-roll.sh install <name>
#   scripts/window-roll.sh uninstall <name>
#   scripts/window-roll.sh status <name>
#
# Window k of the series starts at day k's VARSWAP_ROLL_HOUR:00 UTC and ends 24 h later, with
# buckets every VARSWAP_ROLL_INTERVAL seconds; its address is derived from that spec, so `run`
# is idempotent. One run:
#
# 1. Reads the state of the series' last three windows: the one starting at the next anchor
#    (the first VARSWAP_ROLL_HOUR:00 UTC after now) and the two before it (the running window,
#    and yesterday's while it can still be backfilled). Every read comes before anything is
#    sent: a read error other than "not found" stops the run with nothing changed, so a flaky
#    RPC never creates a duplicate spec nor restarts an agent.
# 2. Creates the window starting at the next anchor only if `show-window` reported it not
#    found. The open windows are the three that exist and are not finalized, counting the one
#    just created.
# 3. If VARSWAP_ROLL_KEEPER names a keeper agent, reinstalls it (scripts/keeper-agent.sh) with
#    those windows when one of them is missing from its plist or the agent is not loaded. The
#    same for VARSWAP_ROLL_WATCH and scripts/watch-agent.sh. An agent that already lists every
#    window is left running.
#
# A reinstall restarts the keeper; it happens about once a day, right after the next window is
# created (a day before it starts). A post cut short by the restart leaves at most one encoded
# VAA, which the keeper's leftover scan closes.
#
# With VARSWAP_ROLL_DRY_RUN=1, `run` prints what it would create and reinstall and changes
# nothing.
#
# `install` writes the LaunchAgent com.varswap.roll.<name>, which runs `run` at load and every
# 6 hours (StartInterval), logging to $VARSWAP_KEEPER_HOME/<name>/roll.log (default home
# ~/.local/share/varswap-keeper). Every run is idempotent, so a missed or repeated run is
# harmless, and with four runs a day the next window exists at least 18 h before it starts.
# It runs this checkout's scripts and target/release binaries (`cargo build --release -p
# varswap-keeper -p varswap-cli` first).
#
# Configuration, taken from the environment and written into the plist, so none of it may hold
# a secret (a public RPC URL; keypair files are referenced by path):
#   SOLANA_URL, VARSWAP_PROGRAM_ID  as for the keeper
#   VARSWAP_KEYPAIR                 signer of create_window; pays the windows' rent
#   VARSWAP_ROLL_FEED               Pyth feed id, hex
#   VARSWAP_ROLL_HOUR               UTC hour of the anchors, 0 to 23 (default 17)
#   VARSWAP_ROLL_INTERVAL           bucket spacing in seconds, dividing 86400 (default 3600)
#   VARSWAP_ROLL_KEEPER             keeper agent name (optional)
#   VARSWAP_ROLL_KEEPER_KEYPAIR     that keeper's payer keypair (required with the keeper)
#   VARSWAP_ROLL_KEEPER_ARGS        passed as VARSWAP_KEEPER_ARGS (optional)
#   VARSWAP_ROLL_WATCH              watcher agent name (optional)
#   VARSWAP_ROLL_WATCH_ARGS         watcher options, e.g. `--payer <address> --grace 600`
#   VARSWAP_WATCH_ALERT_CMD         the watcher's alert command (optional)
#   VARSWAP_ADMIN_BIN               varswap-admin (default target/release/varswap-admin)
#
# For the offline test (scripts/tests/window-roll.sh) only, never written into the plist:
#   VARSWAP_ROLL_SCRIPTS            directory of keeper-agent.sh and watch-agent.sh (default
#                                   this checkout's scripts/)

set -eu

usage() {
	sed -n '6,9p' "$0" | sed 's/^# *//' >&2
	exit 2
}

[ $# -ge 1 ] || usage
cmd=$1
shift
root=$(cd "$(dirname "$0")/.." && pwd)
home=${VARSWAP_KEEPER_HOME:-$HOME/.local/share/varswap-keeper}
day=86400

log() { echo "$(date -u +%Y-%m-%dT%H:%M:%SZ) $*"; }

# Unix time $1 as an ISO 8601 UTC timestamp (BSD date, then GNU date).
iso() {
	date -u -r "$1" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || date -u -d "@$1" +%Y-%m-%dT%H:%M:%SZ
}

# Fails unless $1 is a decimal number.
number() {
	case $1 in '' | *[!0-9]*) echo "$2: not a number: $1" >&2 && exit 2 ;; esac
}

# The window address of the series member starting at $1.
address() {
	"$admin" window-address --feed "$feed" --start "$1" --end $(($1 + day)) --interval "$interval"
}

# Prints the state of window $1 ("pending", "finalized", ...), or "missing" when the account
# does not exist. Fails on any other error; callers run it as `st=$(state ...) || exit 1`.
state() {
	if out=$("$admin" show-window "$1" 2>&1); then
		s=$(printf '%s\n' "$out" | sed -n 's/^ *"state": "\([a-z_]*\)".*/\1/p')
		[ -n "$s" ] || { log "show-window $1: no state in its output" >&2 && exit 1; }
		echo "$s"
	else
		case $out in
		*"account $1 not found"*) echo missing ;;
		*) log "show-window $1 failed, nothing changed: $out" >&2 && exit 1 ;;
		esac
	fi
}

# Exits 0 when agent plist $1 lists every window in $2 and agent $3 is loaded.
current() {
	[ -f "$1" ] || return 1
	for w in $2; do
		grep -q "$w" "$1" || return 1
	done
	launchctl print "gui/$(id -u)/$3" >/dev/null 2>&1 || launchctl print "user/$(id -u)/$3" >/dev/null 2>&1
}

run() {
	: "${SOLANA_URL:?set SOLANA_URL}" "${VARSWAP_PROGRAM_ID:?set VARSWAP_PROGRAM_ID}" \
		"${VARSWAP_KEYPAIR:?set VARSWAP_KEYPAIR}" "${VARSWAP_ROLL_FEED:?set VARSWAP_ROLL_FEED}"
	feed=$VARSWAP_ROLL_FEED
	case $feed in *[!0-9a-fA-F]* | '') echo "VARSWAP_ROLL_FEED: hex only" >&2 && exit 2 ;; esac
	hour=${VARSWAP_ROLL_HOUR:-17}
	interval=${VARSWAP_ROLL_INTERVAL:-3600}
	number "$hour" VARSWAP_ROLL_HOUR
	number "$interval" VARSWAP_ROLL_INTERVAL
	[ "$hour" -le 23 ] || { echo "VARSWAP_ROLL_HOUR: 0 to 23" >&2 && exit 2; }
	[ "$interval" -gt 0 ] && [ $((day % interval)) -eq 0 ] ||
		{ echo "VARSWAP_ROLL_INTERVAL must divide 86400" >&2 && exit 2; }
	admin=${VARSWAP_ADMIN_BIN:-$root/target/release/varswap-admin}
	scripts=${VARSWAP_ROLL_SCRIPTS:-$root/scripts}
	[ -x "$admin" ] || { echo "$admin missing: cargo build --release -p varswap-cli" >&2 && exit 1; }
	dry=${VARSWAP_ROLL_DRY_RUN:-0}

	now=$(date -u +%s)
	next=$((now - now % day + hour * 3600))
	[ "$next" -gt "$now" ] || next=$((next + day))

	addr=$(address "$next")
	# Every read comes before the first transaction or reinstall.
	next_st=$(state "$addr") || exit 1
	open=
	for start in $((next - 2 * day)) $((next - day)); do
		w=$(address "$start")
		st=$(state "$w") || exit 1
		case $st in
		missing | finalized) ;;
		*) open="$open $w" ;;
		esac
	done

	if [ "$next_st" = missing ]; then
		if [ "$dry" = 1 ]; then
			log "dry run: would create $addr starting $(iso "$next")"
		else
			# create-window prints the address and the signature; nothing else is kept.
			out=$("$admin" create-window --feed "$feed" --start "$next" --end $((next + day)) \
				--interval "$interval" 2>&1) || {
				log "create-window $addr failed: $(printf '%s\n' "$out" | grep -E '^(Error|error)' | head -1)" >&2
				exit 1
			}
			log "created $addr starting $(iso "$next"): $(printf '%s\n' "$out" | grep -E '^confirmed ' | head -1)"
		fi
		# Just created (or, in a dry run, would be): open.
		open="$open $addr"
	else
		log "next window $addr exists"
		[ "$next_st" = finalized ] || open="$open $addr"
	fi
	open=${open# }
	log "open windows: ${open:-none}"
	[ -n "$open" ] || return 0

	if [ -n "${VARSWAP_ROLL_KEEPER:-}" ]; then
		k=com.varswap.keeper.$VARSWAP_ROLL_KEEPER
		if current "$HOME/Library/LaunchAgents/$k.plist" "$open" "$k"; then
			log "$k already samples them"
		elif [ "$dry" = 1 ]; then
			log "dry run: would reinstall $k"
		else
			: "${VARSWAP_ROLL_KEEPER_KEYPAIR:?set VARSWAP_ROLL_KEEPER_KEYPAIR}"
			# shellcheck disable=SC2086 # every window is a checked base58 address
			VARSWAP_KEYPAIR=$VARSWAP_ROLL_KEEPER_KEYPAIR VARSWAP_KEEPER_ARGS=${VARSWAP_ROLL_KEEPER_ARGS:-} \
				sh "$scripts/keeper-agent.sh" install "$VARSWAP_ROLL_KEEPER" $open
			log "reinstalled $k"
		fi
	fi
	if [ -n "${VARSWAP_ROLL_WATCH:-}" ]; then
		v=com.varswap.watch.$VARSWAP_ROLL_WATCH
		if current "$HOME/Library/LaunchAgents/$v.plist" "$open" "$v"; then
			log "$v already watches them"
		elif [ "$dry" = 1 ]; then
			log "dry run: would reinstall $v"
		else
			# shellcheck disable=SC2086 # options are word-split on purpose; watch-agent.sh checks them
			sh "$scripts/watch-agent.sh" install "$VARSWAP_ROLL_WATCH" ${VARSWAP_ROLL_WATCH_ARGS:-} $open
			log "reinstalled $v"
		fi
	fi
}

xml() { printf '%s' "$1" | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g'; }

if [ "$cmd" = run ]; then
	run
	exit 0
fi

[ $# -eq 1 ] || usage
name=$1
case $name in
*[!A-Za-z0-9_-]* | '') echo "name must be letters, digits, - or _" >&2 && exit 2 ;;
esac
dir=$home/$name
label=com.varswap.roll.$name
plist=$HOME/Library/LaunchAgents/$label.plist
uid=$(id -u)
if launchctl print "gui/$uid" >/dev/null 2>&1; then
	domain=gui/$uid
else
	domain=user/$uid
fi

loaded() {
	for d in "gui/$uid" "user/$uid"; do
		if launchctl print "$d/$label" >/dev/null 2>&1; then
			echo "$d"
		fi
	done
}

unload() {
	for d in $(loaded); do
		if ! launchctl bootout "$d/$label" 2>/dev/null; then
			if launchctl print "$d/$label" >/dev/null 2>&1; then
				echo "could not unload $label from $d" >&2
				exit 1
			fi
		fi
	done
}

# One <key>/<string> pair per configuration variable that is set.
env_entries() {
	for v in SOLANA_URL VARSWAP_PROGRAM_ID VARSWAP_KEYPAIR VARSWAP_KEEPER_HOME VARSWAP_ADMIN_BIN \
		VARSWAP_ROLL_FEED VARSWAP_ROLL_HOUR VARSWAP_ROLL_INTERVAL VARSWAP_ROLL_KEEPER \
		VARSWAP_ROLL_KEEPER_KEYPAIR VARSWAP_ROLL_KEEPER_ARGS VARSWAP_ROLL_WATCH \
		VARSWAP_ROLL_WATCH_ARGS VARSWAP_WATCH_ALERT_CMD; do
		eval "set=\${$v+x} val=\${$v:-}"
		# shellcheck disable=SC2154 # set and val are assigned by the eval above
		if [ -n "$set" ]; then
			printf '\t\t<key>%s</key><string>%s</string>\n' "$v" "$(xml "$val")"
		fi
	done
}

case $cmd in
install)
	: "${SOLANA_URL:?set SOLANA_URL}" "${VARSWAP_PROGRAM_ID:?set VARSWAP_PROGRAM_ID}" \
		"${VARSWAP_KEYPAIR:?set VARSWAP_KEYPAIR}" "${VARSWAP_ROLL_FEED:?set VARSWAP_ROLL_FEED}"
	# A dry run first: it checks the configuration and the RPC before anything is scheduled.
	VARSWAP_ROLL_DRY_RUN=1 sh "$0" run
	mkdir -p "$dir" "$HOME/Library/LaunchAgents"
	cat >"$plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key><string>$label</string>
	<key>ProgramArguments</key>
	<array>
		<string>/bin/sh</string>
		<string>$(xml "$root/scripts/window-roll.sh")</string>
		<string>run</string>
	</array>
	<key>EnvironmentVariables</key>
	<dict>
$(env_entries)
	</dict>
	<key>LimitLoadToSessionType</key>
	<array>
		<string>Aqua</string>
		<string>Background</string>
	</array>
	<key>RunAtLoad</key><true/>
	<key>StartInterval</key><integer>21600</integer>
	<key>StandardOutPath</key><string>$(xml "$dir/roll.log")</string>
	<key>StandardErrorPath</key><string>$(xml "$dir/roll.log")</string>
	<key>WorkingDirectory</key><string>$(xml "$dir")</string>
</dict>
</plist>
EOF
	plutil -lint "$plist" >/dev/null
	unload
	launchctl bootstrap "$domain" "$plist"
	echo "installed $label: $plist"
	echo "log: $dir/roll.log"
	;;
uninstall)
	unload
	rm -f "$plist"
	echo "uninstalled $label (its log, if any, stays in $dir)"
	;;
status)
	domains=$(loaded)
	if [ -z "$domains" ]; then
		echo "$label not loaded"
	fi
	for d in $domains; do
		echo "$d/$label:"
		launchctl print "$d/$label" | grep -E '^	(state|runs|last exit code) =' || true
	done
	;;
*) usage ;;
esac
