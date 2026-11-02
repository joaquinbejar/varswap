#!/bin/sh
# Runs varswap-watch under launchd (macOS), next to the keepers it watches and independent of
# them, so a dead keeper is noticed (docs/PLAN-improvements.md, I6).
#
#   scripts/watch-agent.sh install <name> [watch options] <window> [<window> ...]
#   scripts/watch-agent.sh uninstall <name>
#   scripts/watch-agent.sh status <name>
#
# `install` copies target/release/varswap-watch (VARSWAP_WATCH_BIN overrides it) to
# $VARSWAP_KEEPER_HOME/<name>/ (default ~/.local/share/varswap-keeper), writes the LaunchAgent
# com.varswap.watch.<name> and loads it with `--interval 60` and the given options (for
# example `--payer <address> --grace 600`; letters, digits, spaces, `-` and `_` only). It runs
# always: restarted after any exit, at most once a minute, and at every login.
#
# Configuration: SOLANA_URL and VARSWAP_PROGRAM_ID from the environment, written into the
# plist (no secret: the watcher holds no key). VARSWAP_WATCH_ALERT_CMD, if set, is the shell
# command run for each alert, with the alert in VARSWAP_ALERT_KIND, VARSWAP_ALERT_SUBJECT and
# VARSWAP_ALERT_MESSAGE; it is written into the plist too. The JSON lines go to
# <home>/<name>/watch.jsonl and errors to <home>/<name>/watch.log.

set -eu

usage() {
	sed -n '5,7p' "$0" | sed 's/^# *//' >&2
	exit 2
}

[ $# -ge 2 ] || usage
cmd=$1
name=$2
shift 2
case $name in
*[!A-Za-z0-9_-]* | '') echo "name must be letters, digits, - or _" >&2 && exit 2 ;;
esac

home=${VARSWAP_KEEPER_HOME:-$HOME/.local/share/varswap-keeper}
dir=$home/$name
label=com.varswap.watch.$name
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

xml() { printf '%s' "$1" | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g'; }

case $cmd in
install)
	[ $# -ge 1 ] || usage
	: "${SOLANA_URL:?set SOLANA_URL}" "${VARSWAP_PROGRAM_ID:?set VARSWAP_PROGRAM_ID}"
	root=$(cd "$(dirname "$0")/.." && pwd)
	bin=${VARSWAP_WATCH_BIN:-$root/target/release/varswap-watch}
	[ -x "$bin" ] || { echo "$bin missing: cargo build --release -p varswap-keeper" >&2 && exit 1; }
	for a in "$@"; do
		case $a in *[!A-Za-z0-9_-]*) echo "options and windows: letters, digits, - and _ only: $a" >&2 && exit 2 ;; esac
	done
	mkdir -p "$dir" "$HOME/Library/LaunchAgents"
	cp "$bin" "$dir/varswap-watch"
	alert=${VARSWAP_WATCH_ALERT_CMD:-}
	cat >"$plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key><string>$label</string>
	<key>ProgramArguments</key>
	<array>
		<string>$(xml "$dir/varswap-watch")</string>
		<string>--interval</string>
		<string>60</string>
$(for a in "$@"; do printf '\t\t<string>%s</string>\n' "$(xml "$a")"; done)
	</array>
	<key>EnvironmentVariables</key>
	<dict>
		<key>SOLANA_URL</key><string>$(xml "$SOLANA_URL")</string>
		<key>VARSWAP_PROGRAM_ID</key><string>$(xml "$VARSWAP_PROGRAM_ID")</string>
		<key>VARSWAP_WATCH_ALERT_CMD</key><string>$(xml "$alert")</string>
	</dict>
	<key>LimitLoadToSessionType</key>
	<array>
		<string>Aqua</string>
		<string>Background</string>
	</array>
	<key>RunAtLoad</key><true/>
	<key>KeepAlive</key><true/>
	<key>ThrottleInterval</key><integer>60</integer>
	<key>StandardOutPath</key><string>$(xml "$dir/watch.jsonl")</string>
	<key>StandardErrorPath</key><string>$(xml "$dir/watch.log")</string>
	<key>WorkingDirectory</key><string>$(xml "$dir")</string>
</dict>
</plist>
EOF
	plutil -lint "$plist" >/dev/null
	unload
	launchctl bootstrap "$domain" "$plist"
	echo "installed $label: $plist"
	echo "status lines: $dir/watch.jsonl, errors: $dir/watch.log"
	;;
uninstall)
	unload
	rm -f "$plist"
	echo "uninstalled $label (its files, if any, stay in $dir)"
	;;
status)
	domains=$(loaded)
	if [ -z "$domains" ]; then
		echo "$label not loaded"
	fi
	for d in $domains; do
		echo "$d/$label:"
		launchctl print "$d/$label" | grep -E '^	(state|pid|runs|last exit code) =' || true
	done
	;;
*) usage ;;
esac
